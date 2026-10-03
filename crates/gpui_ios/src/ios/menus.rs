//! The application's menus in iPadOS's main menu: the menu bar, and the sheet a held ⌘ shows.
//!
//! `UIMainMenuSystem` builds the main menu through a handler given once with its
//! configuration, when the platform starts running ([`configure`]); each `set_menus` asks it
//! to build again. The handler lays out [`crate::menus`]' description: the first GPUI menu
//! after "About" in the application menu, a menu titled like one of UIKit's own (File, Edit,
//! View…) at the start of that menu, any other as a menu of its own before Window. The group
//! UIKit would add for new scenes, documents, printing, finding, the toolbar, the sidebar, the
//! inspector and text formatting is left out, as an application's menus are its own on the Mac
//! too, and a standard edit command a GPUI item stands for replaces UIKit's.
//!
//! Every item is a `UICommand` (a `UIKeyCommand` when it shows a chord) whose action,
//! `gpuiMenuCommand:`, goes up the responder chain to `GPUIViewController`, with its index in
//! the actions of the last build as its property list. A `set_menus` waits for the build
//! before its actions replace those, so a command UIKit still shows always finds its own.
//! UIKit asks the controller's `canPerformAction:` whether one is enabled, which GPUI answers
//! with the action's availability, as `validateMenuItem:` does on the Mac. A command picked in
//! the menu bar dispatches its action. A key command matched by a press is matched before the
//! focused view hears it, so the press goes to the window as the keystroke it was: the keymap
//! then decides in the focused context, exactly as without the menu, and a remote window or a
//! terminal still gets a chord it binds itself.

use std::cell::RefCell;
use std::ptr::NonNull;

use block2::RcBlock;
use gpui::{Action, Keymap, Keystroke, Menu, OwnedMenu, PlatformWindow as _};
use objc2::rc::Retained;
use objc2::runtime::{AnyObject, Bool, ProtocolObject, Sel};
use objc2::{MainThreadMarker, MainThreadOnly as _, Message as _, sel};
use objc2_foundation::{NSArray, NSNumber, NSString};
use objc2_ui_kit::{
    UICommand, UIKeyCommand, UIKeyModifierFlags, UIMainMenuSystem, UIMainMenuSystemConfiguration,
    UIMenu, UIMenuBuilder, UIMenuElement, UIMenuElementAttributes, UIMenuElementState,
    UIMenuIdentifier, UIMenuOptions, UIMenuSystemElementGroupPreference,
};

use super::window::IosWindow;
use crate::hardware_keyboard::{ALTERNATE, COMMAND, CONTROL, SHIFT};
use crate::menus::{
    Command, EditCommand, Element, KeyChord, KeyInput, NamedInput, Place, Standard, TopMenu,
};

// The description's chords use the hardware keyboard's flags, which are UIKit's.
const _: () = assert!(
    COMMAND as isize == UIKeyModifierFlags::Command.bits()
        && CONTROL as isize == UIKeyModifierFlags::Control.bits()
        && ALTERNATE as isize == UIKeyModifierFlags::Alternate.bits()
        && SHIFT as isize == UIKeyModifierFlags::Shift.bits()
);

/// One command of a build: what its index names.
struct Entry {
    action: Box<dyn Action>,
    /// What a press of its chord delivers.
    keystroke: Option<Keystroke>,
    /// Greyed by the app, whatever the action's availability.
    disabled: bool,
}

/// A description of the menus and the commands it holds, by index.
#[derive(Default)]
struct Layout {
    menus: Vec<TopMenu>,
    entries: Vec<Entry>,
}

/// What the menus hold and what GPUI asked to hear. Main thread only, as UIKit's menus are.
#[derive(Default)]
struct State {
    /// What the last `set_menus` gave, until the main menu is built from it.
    pending: Option<Layout>,
    /// What the main menu was last built from: the commands UIKit holds index this.
    built: Layout,
    owned: Option<Vec<OwnedMenu>>,
    perform: Option<Box<dyn FnMut(&dyn Action)>>,
    validate: Option<Box<dyn FnMut(&dyn Action) -> bool>>,
    configured: bool,
    #[cfg(feature = "test-support")]
    built_identifiers: Vec<String>,
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
    let Some(mtm) = MainThreadMarker::new() else {
        log::error!("GPUI iOS: set_menus off the main thread");
        return;
    };
    if STATE.with_borrow(|state| state.configured) {
        UIMainMenuSystem::sharedSystem(mtm).setNeedsRebuild();
    } else {
        configure_on(mtm);
    }
}

/// Keep `menus`, described, for the next build.
fn set_state(menus: Vec<Menu>, keymap: &Keymap) {
    let mut actions = Vec::new();
    let top = crate::menus::describe(&menus, keymap, &mut actions);
    let mut entries: Vec<Entry> = actions
        .into_iter()
        .map(|action| Entry {
            action,
            keystroke: None,
            disabled: false,
        })
        .collect();
    for command in top.iter().flat_map(TopMenu::commands) {
        if let Some(entry) = entries.get_mut(command.index) {
            entry.keystroke = command.keystroke.clone();
            entry.disabled = command.disabled;
        }
    }
    let owned = menus.into_iter().map(Menu::owned).collect();
    STATE.with_borrow_mut(|state| {
        state.pending = Some(Layout {
            menus: top,
            entries,
        });
        state.owned = Some(owned);
    });
}

/// Take what the last `set_menus` gave as what the main menu is built from, and return its
/// menus.
fn lay_out() -> Vec<TopMenu> {
    STATE.with_borrow_mut(|state| {
        if let Some(pending) = state.pending.take() {
            state.built = pending;
        }
        state.built.menus.clone()
    })
}

/// Give the main menu system its configuration and build handler, once. Apple asks for this
/// as early as possible: the platform does it as it starts running, before GPUI holds the app,
/// so the build it sets off never runs inside a GPUI update.
pub(crate) fn configure() {
    match MainThreadMarker::new() {
        Some(mtm) => configure_on(mtm),
        None => log::error!("GPUI iOS: the main menu configured off the main thread"),
    }
}

fn configure_on(mtm: MainThreadMarker) {
    if STATE.with_borrow_mut(|state| std::mem::replace(&mut state.configured, true)) {
        return;
    }
    let configuration = UIMainMenuSystemConfiguration::new(mtm);
    let removed = UIMenuSystemElementGroupPreference::Removed;
    configuration.setNewScenePreference(removed);
    configuration.setDocumentPreference(removed);
    configuration.setPrintingPreference(removed);
    configuration.setFindingPreference(removed);
    configuration.setToolbarPreference(removed);
    configuration.setSidebarPreference(removed);
    configuration.setInspectorPreference(removed);
    configuration.setTextFormattingPreference(removed);
    // UIKit copies the block and calls it on the main thread whenever it builds.
    let handler = RcBlock::new(|builder: NonNull<ProtocolObject<dyn UIMenuBuilder>>| {
        // SAFETY: UIKit passes a live builder that it keeps for the call.
        build(unsafe { builder.as_ref() });
    });
    UIMainMenuSystem::sharedSystem(mtm)
        .setBuildConfiguration_buildHandler(&configuration, Some(&handler));
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

/// The build handler: lay the kept menus into UIKit's.
fn build(builder: &ProtocolObject<dyn UIMenuBuilder>) {
    let mtm = builder.mtm();
    let menus = lay_out();
    let edits: Vec<EditCommand> = menus
        .iter()
        .flat_map(TopMenu::commands)
        .filter_map(|command| command.os_action)
        .collect();
    // SAFETY: UIKit's exported `UIMenuIdentifier` constants, written once as it loads.
    let (about, application, root, standard_edit, undo_redo, window) = unsafe {
        use objc2_ui_kit as ui;
        (
            ui::UIMenuAbout,
            ui::UIMenuApplication,
            ui::UIMenuRoot,
            ui::UIMenuStandardEdit,
            ui::UIMenuUndoRedo,
            ui::UIMenuWindow,
        )
    };
    if edits
        .iter()
        .any(|e| matches!(e, EditCommand::Undo | EditCommand::Redo))
    {
        builder.removeMenuForIdentifier(undo_redo);
    }
    if edits
        .iter()
        .any(|e| !matches!(e, EditCommand::Undo | EditCommand::Redo))
    {
        builder.removeMenuForIdentifier(standard_edit);
    }
    let has = |identifier: &UIMenuIdentifier| builder.menuForIdentifier(identifier).is_some();
    for (nth, top) in menus.iter().enumerate() {
        let children = elements(&top.children, mtm);
        let identifier = NSString::from_str(&identifier(nth));
        match top.place {
            Place::Application => {
                let inline = menu(
                    "",
                    Some(&identifier),
                    UIMenuOptions::DisplayInline,
                    &children,
                    mtm,
                );
                if has(about) {
                    builder.insertSiblingMenu_afterMenuForIdentifier(&inline, about);
                } else {
                    builder.insertChildMenu_atStartOfMenuForIdentifier(&inline, application);
                }
            }
            Place::Standard(standard) if has(standard_identifier(standard)) => {
                let inline = menu(
                    "",
                    Some(&identifier),
                    UIMenuOptions::DisplayInline,
                    &children,
                    mtm,
                );
                builder.insertChildMenu_atStartOfMenuForIdentifier(
                    &inline,
                    standard_identifier(standard),
                );
            }
            Place::Standard(_) | Place::Own => {
                let own = menu(
                    &top.title,
                    Some(&identifier),
                    UIMenuOptions::empty(),
                    &children,
                    mtm,
                );
                if has(window) {
                    builder.insertSiblingMenu_beforeMenuForIdentifier(&own, window);
                } else {
                    builder.insertChildMenu_atEndOfMenuForIdentifier(&own, root);
                }
            }
        }
    }
    #[cfg(feature = "test-support")]
    {
        let built = (0..menus.len())
            .map(identifier)
            .filter(|identifier| has(&NSString::from_str(identifier)))
            .collect();
        STATE.with_borrow_mut(|state| state.built_identifiers = built);
    }
}

/// The identifier of the `nth` GPUI menu in the main menu.
fn identifier(nth: usize) -> String {
    format!("dev.gpui.menu.{nth}")
}

/// The identifiers of the application's menus the main menu held after its last build. Test
/// builds only.
#[cfg(feature = "test-support")]
pub(crate) fn built() -> Vec<String> {
    STATE.with_borrow(|state| state.built_identifiers.clone())
}

/// The identifier of UIKit's own menu `standard`.
fn standard_identifier(standard: Standard) -> &'static UIMenuIdentifier {
    // SAFETY: UIKit's exported `UIMenuIdentifier` constants, written once as it loads.
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

/// `elements` as UIKit's menu elements.
fn elements(elements: &[Element], mtm: MainThreadMarker) -> Retained<NSArray<UIMenuElement>> {
    let made: Vec<Retained<UIMenuElement>> = elements
        .iter()
        .map(|element| match element {
            Element::Command(command) => self::command(command, mtm),
            Element::Group(children) => Retained::into_super(menu(
                "",
                None,
                UIMenuOptions::DisplayInline,
                &self::elements(children, mtm),
                mtm,
            )),
            Element::Submenu { title, children } => Retained::into_super(menu(
                title,
                None,
                UIMenuOptions::empty(),
                &self::elements(children, mtm),
                mtm,
            )),
        })
        .collect();
    NSArray::from_retained_slice(&made)
}

/// A `UIMenu` of `children`, under `identifier` or one UIKit makes up.
fn menu(
    title: &str,
    identifier: Option<&UIMenuIdentifier>,
    options: UIMenuOptions,
    children: &NSArray<UIMenuElement>,
    mtm: MainThreadMarker,
) -> Retained<UIMenu> {
    UIMenu::menuWithTitle_image_identifier_options_children(
        &NSString::from_str(title),
        None,
        identifier,
        options,
        children,
        mtm,
    )
}

/// A `UIKeyCommand` for `command` when it has a chord, else a `UICommand`.
fn command(command: &Command, mtm: MainThreadMarker) -> Retained<UIMenuElement> {
    let title = NSString::from_str(&command.title);
    let index = NSNumber::numberWithUnsignedInteger(command.index);
    let chord = command
        .chord
        .as_ref()
        .and_then(|KeyChord { input, flags }| Some((key_input(input)?, *flags)));
    let made: Retained<UICommand> = match chord {
        Some((input, flags)) => {
            // SAFETY: the action is `gpuiMenuCommand:`, which the GPUI view controller
            // implements, and the property list an NSNumber, a property-list type.
            let made = unsafe {
                UIKeyCommand::commandWithTitle_image_action_input_modifierFlags_propertyList(
                    &title,
                    None,
                    command_selector(),
                    &input,
                    UIKeyModifierFlags::from_bits_retain(flags as isize),
                    Some(&index),
                    mtm,
                )
            };
            // The chord is the keymap's as written, wherever the keys sit on the layout, and
            // it comes before what the system would do with the keys (moving a caret, say).
            made.setAllowsAutomaticLocalization(false);
            made.setAllowsAutomaticMirroring(false);
            made.setWantsPriorityOverSystemBehavior(true);
            Retained::into_super(made)
        }
        // SAFETY: as for the key command.
        None => unsafe {
            UICommand::commandWithTitle_image_action_propertyList(
                &title,
                None,
                command_selector(),
                Some(&index),
                mtm,
            )
        },
    };
    if command.checked {
        made.setState(UIMenuElementState::On);
    }
    if command.disabled {
        made.setAttributes(UIMenuElementAttributes::Disabled);
    }
    Retained::into_super(made)
}

/// The `input` string of a key command, `None` for a function key UIKit names none for.
fn key_input(input: &KeyInput) -> Option<Retained<NSString>> {
    let name = match input {
        KeyInput::Text(text) => return Some(NSString::from_str(text)),
        KeyInput::Named(name) => *name,
    };
    // SAFETY: UIKit's exported `UIKeyInput…` constants, written once as it loads.
    let named: &'static NSString = unsafe {
        use objc2_ui_kit as ui;
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
            NamedInput::F(n) => [
                ui::UIKeyInputF1,
                ui::UIKeyInputF2,
                ui::UIKeyInputF3,
                ui::UIKeyInputF4,
                ui::UIKeyInputF5,
                ui::UIKeyInputF6,
                ui::UIKeyInputF7,
                ui::UIKeyInputF8,
                ui::UIKeyInputF9,
                ui::UIKeyInputF10,
                ui::UIKeyInputF11,
                ui::UIKeyInputF12,
            ]
            .get(usize::from(n).checked_sub(1)?)?,
        }
    };
    Some(named.retain())
}

/// The index a command carries, when `sender` is one of ours.
fn index_of(sender: Option<&AnyObject>) -> Option<usize> {
    let list = sender?.downcast_ref::<UICommand>()?.propertyList()?;
    Some(list.downcast_ref::<NSNumber>()?.unsignedIntegerValue())
}

/// `canPerformAction:withSender:` for [`command_selector`]: whether the command's action is
/// available now.
pub(crate) fn can_perform(sender: Option<&AnyObject>) -> bool {
    let Some(index) = index_of(sender) else {
        return false;
    };
    let Some(action) = STATE.with_borrow(|state| {
        let entry = state.built.entries.get(index)?;
        (!entry.disabled).then(|| entry.action.boxed_clone())
    }) else {
        return false;
    };
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
pub(crate) fn perform(sender: Option<&AnyObject>, window: Option<&IosWindow>) {
    let Some(index) = index_of(sender) else {
        return;
    };
    let Some((action, keystroke)) = STATE.with_borrow(|state| {
        let entry = state.built.entries.get(index)?;
        Some((entry.action.boxed_clone(), entry.keystroke.clone()))
    }) else {
        return;
    };
    let by_key = sender.is_some_and(|sender| sender.downcast_ref::<UIKeyCommand>().is_some());
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
pub(crate) fn answers(action: Sel, sender: Option<&AnyObject>) -> Option<Bool> {
    (action == command_selector()).then(|| Bool::new(can_perform(sender)))
}

#[cfg(test)]
mod tests {
    use std::cell::Cell;
    use std::rc::Rc;

    use gpui::{KeyBinding, MenuItem};

    use super::*;

    gpui::actions!(ios_menus_test, [NewShell, Left, Quiet, Greyed]);

    /// Test threads are not the main thread, and the tests make menu elements on them.
    fn mtm() -> MainThreadMarker {
        // SAFETY: menus and commands are plain model objects until a menu system shows them;
        // these are made, read and dropped on one test thread and never reach one.
        unsafe { MainThreadMarker::new_unchecked() }
    }

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

    fn child<T: objc2::Message>(array: &NSArray<T>, at: usize) -> Retained<T> {
        array.objectAtIndex(at)
    }

    /// A command with a chord is a `UIKeyCommand` matching it, one without a `UICommand`; both
    /// carry their index, and separators make inline menus.
    #[test]
    fn commands_carry_their_chord_and_index() {
        objc2::rc::autoreleasepool(|_| {
            let top = describe();
            let app = elements(&top[0].children, mtm());
            let new_shell = child(&app, 0);
            let key = new_shell
                .downcast_ref::<UIKeyCommand>()
                .expect("a chord makes a key command");
            assert_eq!(key.input().map(|s| s.to_string()).as_deref(), Some("t"));
            assert_eq!(key.modifierFlags(), UIKeyModifierFlags::Command);
            assert_eq!(key.title().to_string(), "New Shell");
            assert_eq!(index_of(Some(&new_shell)), Some(0));

            let layout = elements(&top[1].children, mtm());
            assert_eq!(layout.count(), 2, "two groups");
            let first = child(&layout, 0);
            let first = first.downcast_ref::<UIMenu>().expect("a group is a menu");
            assert!(first.options().contains(UIMenuOptions::DisplayInline));
            let left = child(&first.children(), 0);
            let left = left.downcast_ref::<UIKeyCommand>().expect("a key command");
            // SAFETY: UIKit's exported constant, written once as it loads.
            let arrow = unsafe { objc2_ui_kit::UIKeyInputLeftArrow };
            assert_eq!(left.input().as_deref(), Some(arrow), "UIKit's own input");
            assert_eq!(
                left.modifierFlags(),
                UIKeyModifierFlags::Command | UIKeyModifierFlags::Alternate
            );

            let second = child(&layout, 1);
            let second = second.downcast_ref::<UIMenu>().expect("a menu").children();
            let quiet = child(&second, 0);
            assert!(
                quiet.downcast_ref::<UIKeyCommand>().is_none(),
                "no chord, a plain command"
            );
            assert_eq!(index_of(Some(&quiet)), Some(2));
            let greyed = child(&second, 1);
            let greyed = greyed.downcast_ref::<UICommand>().expect("a command");
            assert!(
                greyed
                    .attributes()
                    .contains(UIMenuElementAttributes::Disabled)
            );
            assert_eq!(greyed.state(), UIMenuElementState::On);
        });
    }

    /// A pick from the menu asks GPUI whether its action is available and dispatches it; an
    /// item greyed by the app is never available. With no window holding the chord's
    /// modifiers, a key command is a pick too. Menus set again are not used before the main
    /// menu is built from them.
    #[test]
    fn a_pick_validates_and_dispatches_its_action() {
        objc2::rc::autoreleasepool(|_| {
            let keymap = Keymap::new(vec![KeyBinding::new("cmd-t", NewShell, None)]);
            let menus = || {
                vec![
                    Menu::new("App").items([MenuItem::action("New Shell", NewShell)]),
                    Menu::new("More").items([MenuItem::Action {
                        name: "Greyed".into(),
                        action: Box::new(Greyed),
                        os_action: None,
                        checked: false,
                        disabled: true,
                    }]),
                ]
            };
            set_state(menus(), &keymap);
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
            let new_shell = child(&elements(&describe()[0].children, mtm()), 0);
            assert!(!can_perform(Some(&new_shell)), "nothing built yet");
            assert_eq!(asked.get(), 0);

            let built = lay_out();
            let new_shell = child(&elements(&built[0].children, mtm()), 0);
            let greyed = child(&elements(&built[1].children, mtm()), 0);
            assert!(can_perform(Some(&new_shell)));
            assert_eq!(asked.get(), 1);
            assert!(!can_perform(Some(&greyed)), "greyed by the app");
            assert_eq!(asked.get(), 1, "without asking GPUI");
            assert!(!can_perform(None));
            assert_eq!(
                answers(sel!(copy:), Some(&new_shell)),
                None,
                "other actions are UIKit's"
            );
            assert_eq!(
                answers(command_selector(), Some(&new_shell)),
                Some(Bool::YES)
            );

            // Set again with the greyed item first: until the next build, index 0 is still
            // New Shell's.
            set_state(menus().into_iter().rev().collect(), &keymap);
            perform(Some(&new_shell), None);
            assert_eq!(ran.get(), 1);
            lay_out();
            assert!(
                !can_perform(Some(&new_shell)),
                "index 0 is the greyed item now"
            );
        });
    }
}
