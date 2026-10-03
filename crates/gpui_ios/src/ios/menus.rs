//! The application's menus in iPadOS's main menu: the menu bar, and the sheet a held ⌘ shows.
//!
//! `UIMainMenuSystem` builds the main menu through a handler given once with its
//! configuration; each later `set_menus` asks it to build again. The handler lays out
//! [`crate::menus`]' description: the first GPUI menu after "About" in the application menu, a
//! menu titled like one of UIKit's own (File, Edit, View…) at the start of that menu, any other
//! as a menu of its own before Window. The group UIKit would add for new scenes, documents,
//! printing, finding, the toolbar, the sidebar, the inspector and text formatting is left out,
//! as an application's menus are its own on the Mac too, and a standard edit command a GPUI
//! item stands for replaces UIKit's.
//!
//! Every item is a `UICommand` (a `UIKeyCommand` when it shows a chord) whose action,
//! `gpuiMenuCommand:`, goes up the responder chain to `GPUIViewController`, with its index in
//! the kept actions as its property list. UIKit asks the controller's `canPerformAction:` whether
//! one is enabled, which GPUI answers with the action's availability, as `validateMenuItem:`
//! does on the Mac. A command picked in the menu bar dispatches its action. A key command
//! matched by a press is matched before the focused view hears it, so the press goes to the
//! window as the keystroke it was: the keymap then decides in the focused context, exactly as
//! without the menu, and a remote window or a terminal still gets a chord it binds itself.

use std::cell::RefCell;

use block2::RcBlock;
use gpui::{Action, Keymap, Keystroke, Menu, OwnedMenu, PlatformWindow as _};
use objc2::runtime::{AnyClass, AnyObject, Bool, Sel};
use objc2::{class, msg_send, sel};
use objc2_foundation::NSString;

use super::util::nsstring;
use super::window::IosWindow;
use crate::menus::{
    Command, EditCommand, Element, KeyChord, KeyInput, NamedInput, Place, Standard, TopMenu,
};

/// `UIMenuOptionsDisplayInline`.
const DISPLAY_INLINE: usize = 1;
/// `UIMenuElementAttributesDisabled`.
const DISABLED: usize = 1;
/// `UIMenuElementStateOn`.
const STATE_ON: isize = 1;
/// `UIMenuSystemElementGroupPreferenceRemoved`.
const REMOVED: isize = 1;

/// What the menus hold and what GPUI asked to hear. Main thread only, as UIKit's menus are.
#[derive(Default)]
struct State {
    menus: Vec<TopMenu>,
    actions: Vec<Box<dyn Action>>,
    /// Per command index: the keystroke its chord delivers, and whether it is always greyed.
    commands: Vec<(Option<Keystroke>, bool)>,
    owned: Option<Vec<OwnedMenu>>,
    perform: Option<Box<dyn FnMut(&dyn Action)>>,
    validate: Option<Box<dyn FnMut(&dyn Action) -> bool>>,
    configured: bool,
    #[cfg(feature = "test-support")]
    built: Vec<String>,
}

thread_local! {
    static STATE: RefCell<State> = RefCell::new(State::default());
}

/// The selector every command sends up the responder chain.
pub(crate) fn command_selector() -> Sel {
    sel!(gpuiMenuCommand:)
}

/// `Platform::set_menus`: keep `menus` and have the main menu built from them.
pub(crate) fn set(menus: Vec<Menu>, keymap: &Keymap) {
    set_state(menus, keymap);
    rebuild();
}

/// Keep `menus`, described.
fn set_state(menus: Vec<Menu>, keymap: &Keymap) {
    let mut actions = Vec::new();
    let top = crate::menus::describe(&menus, keymap, &mut actions);
    let mut commands = vec![(None, false); actions.len()];
    for command in top.iter().flat_map(TopMenu::commands) {
        if let Some(slot) = commands.get_mut(command.index) {
            *slot = (command.keystroke.clone(), command.disabled);
        }
    }
    let owned = menus.into_iter().map(Menu::owned).collect();
    STATE.with_borrow_mut(|state| {
        state.menus = top;
        state.actions = actions;
        state.commands = commands;
        state.owned = Some(owned);
    });
}

/// Hand the main menu system its configuration the first time, else ask it to build again.
fn rebuild() {
    // SAFETY: UIKit's menu system is main-thread only: GPUI calls `set_menus` on its foreground
    // (main) thread, which `isMainThread` confirms before anything is sent.
    unsafe {
        let on_main: bool = msg_send![class!(NSThread), isMainThread];
        if !on_main {
            log::error!("GPUI iOS: set_menus off the main thread");
            return;
        }
        let Some(system_class) = AnyClass::get(c"UIMainMenuSystem") else {
            log::warn!("GPUI iOS: no UIMainMenuSystem here; the menus stay UIKit's");
            return;
        };
        let system: *mut AnyObject = msg_send![system_class, sharedSystem];
        if system.is_null() {
            return;
        }
        if STATE.with_borrow_mut(|state| std::mem::replace(&mut state.configured, true)) {
            let _: () = msg_send![system, setNeedsRebuild];
        } else {
            configure(system);
        }
    }
}

/// `Platform::get_menus`.
pub(crate) fn get() -> Option<Vec<OwnedMenu>> {
    STATE.with_borrow(|state| state.owned.clone())
}

/// `Platform::on_app_menu_action`.
pub(crate) fn on_perform(callback: Box<dyn FnMut(&dyn Action)>) {
    STATE.with_borrow_mut(|state| state.perform = Some(callback));
}

/// `Platform::on_validate_app_menu_command`.
pub(crate) fn on_validate(callback: Box<dyn FnMut(&dyn Action) -> bool>) {
    STATE.with_borrow_mut(|state| state.validate = Some(callback));
}

/// Give `system` the configuration and the build handler, once.
///
/// # Safety
/// Main thread, `system` the shared `UIMainMenuSystem`.
unsafe fn configure(system: *mut AnyObject) {
    // SAFETY: the caller is on the main thread; `UIMainMenuSystemConfiguration` is a plain
    // NSObject made with `new` (+1) and released once the system has copied it (NSCopying).
    unsafe {
        let configuration: *mut AnyObject = msg_send![class!(UIMainMenuSystemConfiguration), new];
        let _: () = msg_send![configuration, setNewScenePreference: REMOVED];
        let _: () = msg_send![configuration, setDocumentPreference: REMOVED];
        let _: () = msg_send![configuration, setPrintingPreference: REMOVED];
        let _: () = msg_send![configuration, setFindingPreference: REMOVED];
        let _: () = msg_send![configuration, setToolbarPreference: REMOVED];
        let _: () = msg_send![configuration, setSidebarPreference: REMOVED];
        let _: () = msg_send![configuration, setInspectorPreference: REMOVED];
        let _: () = msg_send![configuration, setTextFormattingPreference: REMOVED];
        // UIKit copies the block and calls it on the main thread whenever it rebuilds.
        let handler = RcBlock::new(|builder: *mut AnyObject| {
            if !builder.is_null() {
                build(builder);
            }
        });
        let _: () = msg_send![
            system,
            setBuildConfiguration: configuration,
            buildHandler: &*handler
        ];
        let _: () = msg_send![configuration, release];
    }
}

/// The build handler: lay the kept menus into UIKit's.
fn build(builder: *mut AnyObject) {
    // Cloned out, so a validation UIKit runs while building finds the state free.
    let menus = STATE.with_borrow(|state| state.menus.clone());
    let edits: Vec<EditCommand> = menus
        .iter()
        .flat_map(TopMenu::commands)
        .filter_map(|command| command.os_action)
        .collect();
    // SAFETY: the build handler runs on the main thread with a live `UIMenuBuilder`; every
    // identifier is a UIKit `UIMenuIdentifier` constant or an `NSString` made here, and every
    // menu passed is a `UIMenu` made by `menu`.
    unsafe {
        use objc2_ui_kit::{
            UIMenuAbout, UIMenuApplication, UIMenuRoot, UIMenuStandardEdit, UIMenuUndoRedo,
            UIMenuWindow,
        };
        if edits
            .iter()
            .any(|e| matches!(e, EditCommand::Undo | EditCommand::Redo))
        {
            let _: () = msg_send![builder, removeMenuForIdentifier: UIMenuUndoRedo];
        }
        if edits
            .iter()
            .any(|e| !matches!(e, EditCommand::Undo | EditCommand::Redo))
        {
            let _: () = msg_send![builder, removeMenuForIdentifier: UIMenuStandardEdit];
        }
        let has = |identifier: &NSString| -> bool {
            let menu: *mut AnyObject = msg_send![builder, menuForIdentifier: identifier];
            !menu.is_null()
        };
        for (nth, top) in menus.iter().enumerate() {
            let children = elements(&top.children, top.disabled);
            let identifier = format!("dev.gpui.menu.{nth}");
            match top.place {
                Place::Application => {
                    let inline = menu("", Some(&identifier), DISPLAY_INLINE, children);
                    if has(UIMenuAbout) {
                        let _: () = msg_send![builder, insertSiblingMenu: inline, afterMenuForIdentifier: UIMenuAbout];
                    } else {
                        let _: () = msg_send![builder, insertChildMenu: inline, atStartOfMenuForIdentifier: UIMenuApplication];
                    }
                }
                Place::Standard(standard) if has(standard_identifier(standard)) => {
                    let inline = menu("", Some(&identifier), DISPLAY_INLINE, children);
                    let parent = standard_identifier(standard);
                    let _: () = msg_send![builder, insertChildMenu: inline, atStartOfMenuForIdentifier: parent];
                }
                Place::Standard(_) | Place::Own => {
                    let own = menu(&top.title, Some(&identifier), 0, children);
                    if has(UIMenuWindow) {
                        let _: () = msg_send![builder, insertSiblingMenu: own, beforeMenuForIdentifier: UIMenuWindow];
                    } else {
                        let _: () = msg_send![builder, insertChildMenu: own, atEndOfMenuForIdentifier: UIMenuRoot];
                    }
                }
            }
        }
        #[cfg(feature = "test-support")]
        {
            let built = (0..menus.len())
                .map(|nth| format!("dev.gpui.menu.{nth}"))
                .filter(|identifier| has(&NSString::from_str(identifier)))
                .collect();
            STATE.with_borrow_mut(|state| state.built = built);
        }
    }
}

/// The identifiers of the GPUI menus the main menu holds after its last build. Test builds only.
#[cfg(feature = "test-support")]
pub(crate) fn built() -> Vec<String> {
    STATE.with_borrow(|state| state.built.clone())
}

/// The identifier of UIKit's own menu `standard`.
fn standard_identifier(standard: Standard) -> &'static NSString {
    // SAFETY: UIKit's exported `UIMenuIdentifier` constants, which are never written.
    unsafe {
        match standard {
            Standard::File => objc2_ui_kit::UIMenuFile,
            Standard::Edit => objc2_ui_kit::UIMenuEdit,
            Standard::Format => objc2_ui_kit::UIMenuFormat,
            Standard::View => objc2_ui_kit::UIMenuView,
            Standard::Window => objc2_ui_kit::UIMenuWindow,
            Standard::Help => objc2_ui_kit::UIMenuHelp,
        }
    }
}

/// `elements` as an `NSArray` of `UIMenuElement`s, every command greyed when `disabled`.
fn elements(elements: &[Element], disabled: bool) -> *mut AnyObject {
    let made: Vec<*mut AnyObject> = elements
        .iter()
        .map(|element| match element {
            Element::Command(command) => self::command(command, disabled),
            Element::Group(children) => {
                menu("", None, DISPLAY_INLINE, self::elements(children, disabled))
            }
            Element::Submenu {
                title,
                disabled: own,
                children,
            } => menu(title, None, 0, self::elements(children, disabled || *own)),
        })
        .collect();
    // SAFETY: every pointer is a live autoreleased UIMenuElement; `arrayWithObjects:count:`
    // retains them into an autoreleased array.
    unsafe {
        msg_send![
            class!(NSArray),
            arrayWithObjects: made.as_ptr(),
            count: made.len()
        ]
    }
}

/// An autoreleased `UIMenu` of `children` (an `NSArray`), under `identifier` or one UIKit
/// makes up.
fn menu(
    title: &str,
    identifier: Option<&str>,
    options: usize,
    children: *mut AnyObject,
) -> *mut AnyObject {
    // SAFETY: main thread (menus are built and set only there); `menuWithTitle:…` returns an
    // autoreleased UIMenu, and a nil image and identifier are allowed.
    unsafe {
        let identifier = identifier.map_or(std::ptr::null_mut(), |id| nsstring(id));
        msg_send![
            class!(UIMenu),
            menuWithTitle: nsstring(title),
            image: std::ptr::null_mut::<AnyObject>(),
            identifier: identifier,
            options: options,
            children: children
        ]
    }
}

/// An autoreleased `UIKeyCommand` for `command` when it has a chord, else a `UICommand`.
fn command(command: &Command, disabled: bool) -> *mut AnyObject {
    // SAFETY: main thread; both constructors return autoreleased commands, a nil image is
    // allowed, and the property list is an NSNumber, which is a property-list type.
    unsafe {
        let title = nsstring(&command.title);
        let index: *mut AnyObject =
            msg_send![class!(NSNumber), numberWithUnsignedInteger: command.index];
        let action = command_selector();
        let image = std::ptr::null_mut::<AnyObject>();
        let made: *mut AnyObject = match &command.chord {
            Some(KeyChord { input, flags }) => {
                let made: *mut AnyObject = msg_send![
                    class!(UIKeyCommand),
                    commandWithTitle: title,
                    image: image,
                    action: action,
                    input: key_input(input),
                    modifierFlags: *flags as isize,
                    propertyList: index
                ];
                // The chord is the keymap's as written, wherever the keys sit on the layout.
                let _: () = msg_send![made, setAllowsAutomaticLocalization: false];
                let _: () = msg_send![made, setAllowsAutomaticMirroring: false];
                let _: () = msg_send![made, setWantsPriorityOverSystemBehavior: true];
                made
            }
            None => msg_send![
                class!(UICommand),
                commandWithTitle: title,
                image: image,
                action: action,
                propertyList: index
            ],
        };
        if command.checked {
            let _: () = msg_send![made, setState: STATE_ON];
        }
        if disabled || command.disabled {
            let _: () = msg_send![made, setAttributes: DISABLED];
        }
        made
    }
}

/// The `input` string of a key command.
fn key_input(input: &KeyInput) -> *mut AnyObject {
    let named = |name: NamedInput| -> &'static NSString {
        use objc2_ui_kit as ui;
        // SAFETY: UIKit's exported `UIKeyInput…` constants, which are never written.
        unsafe {
            match name {
                NamedInput::Up => ui::UIKeyInputUpArrow,
                NamedInput::Down => ui::UIKeyInputDownArrow,
                NamedInput::Left => ui::UIKeyInputLeftArrow,
                NamedInput::Right => ui::UIKeyInputRightArrow,
                NamedInput::Escape => ui::UIKeyInputEscape,
                NamedInput::PageUp => ui::UIKeyInputPageUp,
                NamedInput::PageDown => ui::UIKeyInputPageDown,
                NamedInput::Home => ui::UIKeyInputHome,
                NamedInput::End => ui::UIKeyInputEnd,
                NamedInput::Delete => ui::UIKeyInputDelete,
                NamedInput::F(1) => ui::UIKeyInputF1,
                NamedInput::F(2) => ui::UIKeyInputF2,
                NamedInput::F(3) => ui::UIKeyInputF3,
                NamedInput::F(4) => ui::UIKeyInputF4,
                NamedInput::F(5) => ui::UIKeyInputF5,
                NamedInput::F(6) => ui::UIKeyInputF6,
                NamedInput::F(7) => ui::UIKeyInputF7,
                NamedInput::F(8) => ui::UIKeyInputF8,
                NamedInput::F(9) => ui::UIKeyInputF9,
                NamedInput::F(10) => ui::UIKeyInputF10,
                NamedInput::F(11) => ui::UIKeyInputF11,
                NamedInput::F(_) => ui::UIKeyInputF12,
            }
        }
    };
    match input {
        // SAFETY: inside a UIKit build handler, which has an autorelease pool.
        KeyInput::Text(text) => unsafe { nsstring(text) },
        KeyInput::Named(name) => std::ptr::from_ref(named(*name)).cast_mut().cast(),
    }
}

/// The index a command carries, when `sender` is one of ours.
fn index_of(sender: *mut AnyObject) -> Option<usize> {
    if sender.is_null() {
        return None;
    }
    // SAFETY: main thread; `sender` is the object UIKit passed, checked to be a UICommand
    // before its property list is read, and that checked to be an NSNumber before it is read.
    unsafe {
        let is_command: bool = msg_send![sender, isKindOfClass: class!(UICommand)];
        if !is_command {
            return None;
        }
        let list: *mut AnyObject = msg_send![sender, propertyList];
        if list.is_null() {
            return None;
        }
        let is_number: bool = msg_send![list, isKindOfClass: class!(NSNumber)];
        if !is_number {
            return None;
        }
        let index: usize = msg_send![list, unsignedIntegerValue];
        Some(index)
    }
}

/// `canPerformAction:withSender:` for [`command_selector`]: whether the command's action is
/// available now.
pub(crate) fn can_perform(sender: *mut AnyObject) -> bool {
    let Some(index) = index_of(sender) else {
        return false;
    };
    let Some((action, greyed)) = STATE.with_borrow(|state| {
        let action = state.actions.get(index)?.boxed_clone();
        let greyed = state.commands.get(index).is_some_and(|(_, greyed)| *greyed);
        Some((action, greyed))
    }) else {
        return false;
    };
    if greyed {
        return false;
    }
    let Some(mut validate) = STATE.with_borrow_mut(|state| state.validate.take()) else {
        return false;
    };
    let available = validate(action.as_ref());
    STATE.with_borrow_mut(|state| {
        state.validate.get_or_insert(validate);
    });
    available
}

/// [`command_selector`]'s action, from the controller of `window`: a key command matched by
/// its chord goes to the window as that keystroke, a command picked from the menu dispatches
/// its action.
pub(crate) fn perform(sender: *mut AnyObject, window: Option<&IosWindow>) {
    let Some(index) = index_of(sender) else {
        return;
    };
    let Some((action, keystroke)) = STATE.with_borrow(|state| {
        let action = state.actions.get(index)?.boxed_clone();
        let keystroke = state.commands.get(index).and_then(|(k, _)| k.clone());
        Some((action, keystroke))
    }) else {
        return;
    };
    // SAFETY: main thread; `sender` is the UICommand UIKit passed (checked by `index_of`).
    let by_key: bool = unsafe { msg_send![sender, isKindOfClass: class!(UIKeyCommand)] };
    if by_key
        && let Some(window) = window
        && let Some(keystroke) = keystroke
        && crate::menus::pressed(&keystroke, window.modifiers())
    {
        window.deliver_menu_keystroke(keystroke);
        return;
    }
    let Some(mut perform) = STATE.with_borrow_mut(|state| state.perform.take()) else {
        return;
    };
    perform(action.as_ref());
    STATE.with_borrow_mut(|state| {
        state.perform.get_or_insert(perform);
    });
}

/// The `canPerformAction:withSender:` the controller answers with.
pub(crate) fn answers(action: Sel, sender: *mut AnyObject) -> Option<Bool> {
    (action == command_selector()).then(|| Bool::new(can_perform(sender)))
}

#[cfg(test)]
mod tests {
    use std::cell::Cell;
    use std::rc::Rc;

    use gpui::{KeyBinding, MenuItem};

    use super::*;
    use crate::hardware_keyboard::{ALTERNATE, COMMAND};

    gpui::actions!(ios_menus_test, [NewShell, Left, Quiet, Greyed]);

    fn describe() -> Vec<TopMenu> {
        let keymap = Keymap::new(vec![
            KeyBinding::new("cmd-t", NewShell, None),
            KeyBinding::new("cmd-alt-left", Left, None),
        ]);
        let menus = vec![
            Menu::new("App").items([MenuItem::action("New Shell", NewShell)]),
            Menu::new("Layout").items([
                MenuItem::action("Focus Left", Left),
                MenuItem::separator(),
                MenuItem::action("Quiet", Quiet),
                MenuItem::Action {
                    name: "Greyed".into(),
                    action: Box::new(Greyed),
                    os_action: None,
                    checked: true,
                    disabled: true,
                },
            ]),
        ];
        let mut actions = Vec::new();
        crate::menus::describe(&menus, &keymap, &mut actions)
    }

    fn string(ns: *mut AnyObject) -> String {
        // SAFETY: `ns` is a live NSString the test read off a command; `UTF8String` points into
        // it for as long as it lives.
        unsafe {
            let utf8: *const std::ffi::c_char = msg_send![ns, UTF8String];
            std::ffi::CStr::from_ptr(utf8)
                .to_string_lossy()
                .into_owned()
        }
    }

    fn child(array: *mut AnyObject, at: usize) -> *mut AnyObject {
        // SAFETY: `array` is an NSArray the test made, `at` in its bounds.
        unsafe { msg_send![array, objectAtIndex: at] }
    }

    /// A command with a chord is a `UIKeyCommand` matching it, one without a `UICommand`; both
    /// carry their index, and separators make inline menus.
    #[test]
    fn commands_carry_their_chord_and_index() {
        objc2::rc::autoreleasepool(|_| {
            let top = describe();
            let app = elements(&top[0].children, false);
            let new_shell = child(app, 0);
            // SAFETY: main-thread-free reads of plain UIKit objects the test made.
            unsafe {
                let is_key: bool = msg_send![new_shell, isKindOfClass: class!(UIKeyCommand)];
                assert!(is_key);
                let input: *mut AnyObject = msg_send![new_shell, input];
                assert_eq!(string(input), "t");
                let flags: isize = msg_send![new_shell, modifierFlags];
                assert_eq!(flags, COMMAND as isize);
                let title: *mut AnyObject = msg_send![new_shell, title];
                assert_eq!(string(title), "New Shell");
                assert_eq!(index_of(new_shell), Some(0));

                let layout = elements(&top[1].children, false);
                let count: usize = msg_send![layout, count];
                assert_eq!(count, 2, "two groups");
                let first = child(layout, 0);
                let options: usize = msg_send![first, options];
                assert_eq!(options & DISPLAY_INLINE, DISPLAY_INLINE);
                let left = child(msg_send![first, children], 0);
                let input: *mut AnyObject = msg_send![left, input];
                let arrow: *const NSString = objc2_ui_kit::UIKeyInputLeftArrow;
                let same: bool = msg_send![input, isEqualToString: arrow];
                assert!(same, "the left arrow is UIKit's own input");
                let flags: isize = msg_send![left, modifierFlags];
                assert_eq!(flags, (COMMAND | ALTERNATE) as isize);

                let second: *mut AnyObject = msg_send![child(layout, 1), children];
                let quiet = child(second, 0);
                let is_key: bool = msg_send![quiet, isKindOfClass: class!(UIKeyCommand)];
                assert!(!is_key, "no chord, a plain command");
                assert_eq!(index_of(quiet), Some(2));
                let greyed = child(second, 1);
                let attributes: usize = msg_send![greyed, attributes];
                assert_eq!(attributes & DISABLED, DISABLED);
                let state: isize = msg_send![greyed, state];
                assert_eq!(state, STATE_ON);
            }
        });
    }

    /// A pick from the menu asks GPUI whether its action is available and dispatches it; an
    /// item greyed by the app is never available. With no window holding the chord's
    /// modifiers, a key command is a pick too.
    #[test]
    fn a_pick_validates_and_dispatches_its_action() {
        objc2::rc::autoreleasepool(|_| {
            let keymap = Keymap::new(vec![KeyBinding::new("cmd-t", NewShell, None)]);
            set_state(
                vec![
                    Menu::new("App").items([MenuItem::action("New Shell", NewShell)]),
                    Menu::new("More").items([MenuItem::Action {
                        name: "Greyed".into(),
                        action: Box::new(Greyed),
                        os_action: None,
                        checked: false,
                        disabled: true,
                    }]),
                ],
                &keymap,
            );
            let asked = Rc::new(Cell::new(0));
            let ran = Rc::new(Cell::new(0));
            on_validate(Box::new({
                let asked = asked.clone();
                move |action| {
                    asked.set(asked.get() + 1);
                    action.partial_eq(&NewShell)
                }
            }));
            on_perform(Box::new({
                let ran = ran.clone();
                move |action| {
                    assert!(action.partial_eq(&NewShell));
                    ran.set(ran.get() + 1);
                }
            }));
            let menus = STATE.with_borrow(|state| state.menus.clone());
            let new_shell = child(elements(&menus[0].children, false), 0);
            let greyed = child(elements(&menus[1].children, false), 0);
            assert!(can_perform(new_shell));
            assert_eq!(asked.get(), 1);
            assert!(!can_perform(greyed), "greyed by the app");
            assert_eq!(asked.get(), 1, "without asking GPUI");
            assert!(!can_perform(std::ptr::null_mut()));
            assert_eq!(
                answers(sel!(copy:), new_shell),
                None,
                "other actions are UIKit's"
            );
            assert_eq!(answers(command_selector(), new_shell), Some(Bool::YES));
            perform(new_shell, None);
            assert_eq!(ran.get(), 1);
        });
    }
}
