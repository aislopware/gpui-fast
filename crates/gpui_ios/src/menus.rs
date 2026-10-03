//! The application's menus as iPadOS shows them: in the menu bar, and in the sheet a held ⌘
//! brings up.
//!
//! This is the half that needs no UIKit, so it is compiled and tested on the host too: which
//! GPUI menu goes where in the main menu, how separators become inline groups, and which chord
//! each command shows. `ios::menus` turns the description into `UIMenu`s and `UIKeyCommand`s.
//!
//! A chord is shown, and becomes a key command, only when it holds ⌘, ⌃ or ⌥ and is one
//! keystroke: a bare key or a ⇧-only one stays the focused view's to type, since UIKit matches
//! key commands before the view hears the press.

use gpui::{Action, KeyContext, Keymap, Keystroke, Menu, MenuItem, Modifiers, OsAction};

use crate::hardware_keyboard::{ALTERNATE, COMMAND, CONTROL, SHIFT};

/// A key UIKit names with a `UIKeyInput…` constant rather than a character.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NamedInput {
    /// `UIKeyInputUpArrow`.
    Up,
    /// `UIKeyInputDownArrow`.
    Down,
    /// `UIKeyInputLeftArrow`.
    Left,
    /// `UIKeyInputRightArrow`.
    Right,
    /// `UIKeyInputEscape`.
    Escape,
    /// `UIKeyInputPageUp`.
    PageUp,
    /// `UIKeyInputPageDown`.
    PageDown,
    /// `UIKeyInputHome`.
    Home,
    /// `UIKeyInputEnd`.
    End,
    /// `UIKeyInputDelete`: the forward delete.
    Delete,
    /// `UIKeyInputF1` to `UIKeyInputF12`.
    F(u8),
}

/// What a `UIKeyCommand` matches: its `input`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum KeyInput {
    /// A character, as the key types it unshifted.
    Text(String),
    /// A key UIKit names.
    Named(NamedInput),
}

/// A `UIKeyCommand`'s `input` and `modifierFlags`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct KeyChord {
    /// The key.
    pub input: KeyInput,
    /// `UIKeyModifierFlags`.
    pub flags: u32,
}

/// The chord a key command matches for `keystroke`, or `None` when it should stay with the
/// focused view: no ⌘, ⌃ or ⌥ held, or a key UIKit has no input for.
pub fn chord(keystroke: &Keystroke) -> Option<KeyChord> {
    let Modifiers {
        control,
        alt,
        shift,
        platform,
        function: _,
    } = keystroke.modifiers;
    if !(control || alt || platform) {
        return None;
    }
    let input = match keystroke.key.as_str() {
        "up" => KeyInput::Named(NamedInput::Up),
        "down" => KeyInput::Named(NamedInput::Down),
        "left" => KeyInput::Named(NamedInput::Left),
        "right" => KeyInput::Named(NamedInput::Right),
        "escape" => KeyInput::Named(NamedInput::Escape),
        "pageup" => KeyInput::Named(NamedInput::PageUp),
        "pagedown" => KeyInput::Named(NamedInput::PageDown),
        "home" => KeyInput::Named(NamedInput::Home),
        "end" => KeyInput::Named(NamedInput::End),
        "delete" => KeyInput::Named(NamedInput::Delete),
        "enter" => KeyInput::Text("\r".to_owned()),
        "tab" => KeyInput::Text("\t".to_owned()),
        "space" => KeyInput::Text(" ".to_owned()),
        "backspace" => KeyInput::Text("\u{8}".to_owned()),
        key => match key.strip_prefix('f').map(str::parse::<u8>) {
            Some(Ok(n @ 1..=12)) => KeyInput::Named(NamedInput::F(n)),
            _ if key.chars().count() == 1 => KeyInput::Text(key.to_owned()),
            _ => return None,
        },
    };
    let flags = [
        (platform, COMMAND),
        (control, CONTROL),
        (alt, ALTERNATE),
        (shift, SHIFT),
    ]
    .into_iter()
    .filter(|(held, _)| *held)
    .fold(0, |flags, (_, flag)| flags | flag);
    Some(KeyChord { input, flags })
}

/// The keystroke a menu item for `action` shows: the same binding the macOS menu bar shows.
/// Earlier bindings win here, unlike most displays (zed #23621), and one whose context the
/// default editor context satisfies wins over the first.
pub fn keystroke_for(action: &dyn Action, keymap: &Keymap) -> Option<Keystroke> {
    let context = default_context();
    let mut first = None;
    let mut found = None;
    for binding in keymap.bindings_for_action(action) {
        if first.is_none() {
            first = Some(binding);
        }
        if binding
            .predicate()
            .is_none_or(|predicate| predicate.eval(std::slice::from_ref(&context)))
        {
            found = Some(binding);
            break;
        }
    }
    match found.or(first)?.keystrokes() {
        [keystroke] => Some(keystroke.inner().clone()),
        _ => None,
    }
}

/// The context `gpui_macos` evaluates a binding's predicate in for its menu bar.
fn default_context() -> KeyContext {
    let mut workspace = KeyContext::new_with_defaults();
    workspace.add("Workspace");
    let mut pane = KeyContext::new_with_defaults();
    pane.add("Pane");
    let mut editor = KeyContext::new_with_defaults();
    editor.add("Editor");
    pane.extend(&editor);
    workspace.extend(&pane);
    workspace
}

/// One item of a menu.
#[derive(Clone, Debug, PartialEq)]
pub enum Element {
    /// An action: a `UIKeyCommand` when it has a chord, else a `UICommand`.
    Command(Command),
    /// The items between two separators, drawn inline with a line around them.
    Group(Vec<Element>),
    /// A submenu.
    Submenu {
        /// Its title.
        title: String,
        /// Whether it is greyed out.
        disabled: bool,
        /// What it holds.
        children: Vec<Element>,
    },
}

/// A menu item that runs a GPUI action.
#[derive(Clone, Debug, PartialEq)]
pub struct Command {
    /// What it says.
    pub title: String,
    /// Where its action is kept: the `propertyList` the command carries.
    pub index: usize,
    /// The chord it shows and matches, when it has one of its own.
    pub chord: Option<KeyChord>,
    /// The keystroke the chord was made from: what a press of it delivers.
    pub keystroke: Option<Keystroke>,
    /// Shown with a check mark.
    pub checked: bool,
    /// Greyed out whatever the action's availability.
    pub disabled: bool,
    /// The standard edit command it stands for, whose UIKit twin it replaces.
    pub os_action: Option<EditCommand>,
}

/// A standard edit command a GPUI menu item can stand for ([`OsAction`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EditCommand {
    /// `cut:`.
    Cut,
    /// `copy:`.
    Copy,
    /// `paste:`.
    Paste,
    /// `selectAll:`.
    SelectAll,
    /// `undo:`.
    Undo,
    /// `redo:`.
    Redo,
}

impl From<OsAction> for EditCommand {
    fn from(action: OsAction) -> Self {
        match action {
            OsAction::Cut => Self::Cut,
            OsAction::Copy => Self::Copy,
            OsAction::Paste => Self::Paste,
            OsAction::SelectAll => Self::SelectAll,
            OsAction::Undo => Self::Undo,
            OsAction::Redo => Self::Redo,
        }
    }
}

/// A main-menu menu UIKit already has, which a GPUI menu of the same title fills.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Standard {
    /// `UIMenuFile`.
    File,
    /// `UIMenuEdit`.
    Edit,
    /// `UIMenuFormat`.
    Format,
    /// `UIMenuView`.
    View,
    /// `UIMenuWindow`.
    Window,
    /// `UIMenuHelp`.
    Help,
}

/// Where a top-level GPUI menu goes in the main menu.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Place {
    /// The application menu, after "About": the first GPUI menu, as on the Mac.
    Application,
    /// At the start of the UIKit menu of the same title.
    Standard(Standard),
    /// A menu of its own, before the Window menu, in the order given.
    Own,
}

/// A top-level menu.
#[derive(Clone, Debug, PartialEq)]
pub struct TopMenu {
    /// Its title.
    pub title: String,
    /// Where it goes.
    pub place: Place,
    /// Whether it is greyed out.
    pub disabled: bool,
    /// What it holds.
    pub children: Vec<Element>,
}

impl TopMenu {
    /// Every command in it, depth first.
    pub fn commands(&self) -> Vec<&Command> {
        fn walk<'a>(elements: &'a [Element], out: &mut Vec<&'a Command>) {
            for element in elements {
                match element {
                    Element::Command(command) => out.push(command),
                    Element::Group(children) | Element::Submenu { children, .. } => {
                        walk(children, out);
                    }
                }
            }
        }
        let mut out = Vec::new();
        walk(&self.children, &mut out);
        out
    }
}

/// The standard menu a GPUI menu titled `title` fills, if any.
fn standard(title: &str) -> Option<Standard> {
    Some(match title.to_ascii_lowercase().as_str() {
        "file" => Standard::File,
        "edit" => Standard::Edit,
        "format" => Standard::Format,
        "view" => Standard::View,
        "window" => Standard::Window,
        "help" => Standard::Help,
        _ => return None,
    })
}

/// Describe `menus` for the main menu, pushing each action onto `actions` at its command's
/// index. A chord already taken by an earlier command is left off a later one, since UIKit
/// would match only one of them.
pub fn describe(
    menus: &[Menu],
    keymap: &Keymap,
    actions: &mut Vec<Box<dyn Action>>,
) -> Vec<TopMenu> {
    let mut taken = Vec::new();
    menus
        .iter()
        .enumerate()
        .map(|(nth, menu)| {
            let place = if nth == 0 {
                Place::Application
            } else {
                standard(&menu.name).map_or(Place::Own, Place::Standard)
            };
            TopMenu {
                title: menu.name.to_string(),
                place,
                disabled: menu.disabled,
                children: items(&menu.items, keymap, actions, &mut taken),
            }
        })
        .collect()
}

/// A menu's items, cut into inline groups at its separators when it has any.
fn items(
    items: &[MenuItem],
    keymap: &Keymap,
    actions: &mut Vec<Box<dyn Action>>,
    taken: &mut Vec<KeyChord>,
) -> Vec<Element> {
    let mut groups: Vec<Vec<Element>> = vec![Vec::new()];
    for item in items {
        match item {
            MenuItem::Separator => groups.push(Vec::new()),
            // The Services menu and its like are the Mac's own.
            MenuItem::SystemMenu(_) => {}
            MenuItem::Submenu(menu) => {
                let children = self::items(&menu.items, keymap, actions, taken);
                if let Some(group) = groups.last_mut() {
                    group.push(Element::Submenu {
                        title: menu.name.to_string(),
                        disabled: menu.disabled,
                        children,
                    });
                }
            }
            MenuItem::Action {
                name,
                action,
                os_action,
                checked,
                disabled,
            } => {
                let keystroke = keystroke_for(action.as_ref(), keymap);
                let chord = keystroke
                    .as_ref()
                    .and_then(chord)
                    .filter(|chord| !taken.contains(chord));
                if let Some(chord) = &chord {
                    taken.push(chord.clone());
                }
                let command = Command {
                    title: name.to_string(),
                    index: actions.len(),
                    keystroke: chord.as_ref().and(keystroke),
                    chord,
                    checked: *checked,
                    disabled: *disabled,
                    os_action: os_action.map(EditCommand::from),
                };
                actions.push(action.boxed_clone());
                if let Some(group) = groups.last_mut() {
                    group.push(Element::Command(command));
                }
            }
        }
    }
    groups.retain(|group| !group.is_empty());
    if groups.len() <= 1 {
        return groups.pop().unwrap_or_default();
    }
    groups.into_iter().map(Element::Group).collect()
}

/// Whether a press of `held` modifiers is the keyboard asking for `keystroke`'s command: the
/// modifiers it was pressed with are the chord's. A pick from the menu bar holds none.
pub fn pressed(keystroke: &Keystroke, held: Modifiers) -> bool {
    let chord = keystroke.modifiers;
    held.control == chord.control
        && held.alt == chord.alt
        && held.shift == chord.shift
        && held.platform == chord.platform
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui::KeyBinding;

    gpui::actions!(test_menus, [NewShell, Palette, Find, Copy, Quiet, Other]);

    fn ks(text: &str) -> Keystroke {
        Keystroke::parse(text).unwrap()
    }

    #[test]
    fn chords_need_a_command_control_or_option() {
        assert_eq!(
            chord(&ks("cmd-shift-p")),
            Some(KeyChord {
                input: KeyInput::Text("p".into()),
                flags: COMMAND | SHIFT
            })
        );
        assert_eq!(
            chord(&ks("ctrl-cmd-shift-p")).map(|c| c.flags),
            Some(COMMAND | CONTROL | SHIFT)
        );
        assert_eq!(chord(&ks("alt-x")).map(|c| c.flags), Some(ALTERNATE));
        assert_eq!(chord(&ks("p")), None, "a bare key is the view's to type");
        assert_eq!(chord(&ks("shift-p")), None, "so is a shifted one");
        assert_eq!(chord(&ks("f5")), None, "and a bare function key");
    }

    #[test]
    fn named_keys_use_uikits_inputs() {
        let input = |text: &str| chord(&ks(text)).map(|c| c.input);
        assert_eq!(
            input("cmd-alt-left"),
            Some(KeyInput::Named(NamedInput::Left))
        );
        assert_eq!(input("cmd-up"), Some(KeyInput::Named(NamedInput::Up)));
        assert_eq!(
            input("ctrl-pagedown"),
            Some(KeyInput::Named(NamedInput::PageDown))
        );
        assert_eq!(
            input("cmd-escape"),
            Some(KeyInput::Named(NamedInput::Escape))
        );
        assert_eq!(input("cmd-f12"), Some(KeyInput::Named(NamedInput::F(12))));
        assert_eq!(input("cmd-enter"), Some(KeyInput::Text("\r".into())));
        assert_eq!(input("cmd-backspace"), Some(KeyInput::Text("\u{8}".into())));
        assert_eq!(input("cmd-,"), Some(KeyInput::Text(",".into())));
        assert_eq!(input("cmd-f"), Some(KeyInput::Text("f".into())));
        assert_eq!(input("cmd-f13"), None, "UIKit names no F13");
    }

    fn keymap() -> Keymap {
        Keymap::new(vec![
            KeyBinding::new("cmd-t", NewShell, None),
            KeyBinding::new("cmd-n", NewShell, None),
            KeyBinding::new("cmd-shift-p", Palette, None),
            KeyBinding::new("ctrl-cmd-shift-p", Palette, Some("Remote")),
            KeyBinding::new("cmd-f", Find, Some("Terminal")),
            KeyBinding::new("cmd-c", Copy, None),
            KeyBinding::new("q", Quiet, None),
            KeyBinding::new("cmd-k cmd-o", Other, None),
        ])
    }

    fn menus() -> Vec<Menu> {
        vec![
            Menu::new("Slopty").items([
                MenuItem::action("New Shell", NewShell),
                MenuItem::os_submenu("Services", gpui::SystemMenuType::Services),
            ]),
            Menu::new("Edit").items([
                MenuItem::os_action("Copy", Copy, OsAction::Copy),
                MenuItem::separator(),
                MenuItem::action("Find…", Find),
                MenuItem::action("Quiet", Quiet),
            ]),
            Menu::new("Layout").items([
                MenuItem::action("Commands…", Palette),
                MenuItem::submenu(Menu::new("More").items([
                    MenuItem::action("Other", Other),
                    MenuItem::action("New Shell Again", NewShell),
                ])),
            ]),
        ]
    }

    #[test]
    fn menus_go_where_ipados_keeps_them() {
        let mut actions = Vec::new();
        let top = describe(&menus(), &keymap(), &mut actions);
        let places: Vec<_> = top.iter().map(|m| (m.title.as_str(), m.place)).collect();
        assert_eq!(
            places,
            [
                ("Slopty", Place::Application),
                ("Edit", Place::Standard(Standard::Edit)),
                ("Layout", Place::Own),
            ]
        );
        assert_eq!(top[0].children.len(), 1, "the Services menu is the Mac's");
        assert_eq!(actions.len(), 7, "one kept action per command");
        for (nth, command) in top.iter().flat_map(TopMenu::commands).enumerate() {
            assert_eq!(command.index, nth, "{} keeps its index", command.title);
            assert!(
                actions[command.index].partial_eq(match command.title.as_str() {
                    "New Shell" | "New Shell Again" => &NewShell,
                    "Copy" => &Copy,
                    "Find…" => &Find,
                    "Quiet" => &Quiet,
                    "Commands…" => &Palette,
                    _ => &Other,
                })
            );
        }
    }

    #[test]
    fn separators_make_inline_groups() {
        let mut actions = Vec::new();
        let top = describe(&menus(), &keymap(), &mut actions);
        let [Element::Group(first), Element::Group(second)] = top[1].children.as_slice() else {
            panic!("two groups: {:?}", top[1].children);
        };
        assert_eq!(first.len(), 1);
        assert_eq!(second.len(), 2);
        assert!(
            matches!(&top[2].children[1], Element::Submenu { title, children, .. } if title == "More" && children.len() == 2),
            "no separator, no group: {:?}",
            top[2].children
        );
    }

    #[test]
    fn each_command_shows_one_chord_and_no_chord_twice() {
        let mut actions = Vec::new();
        let top = describe(&menus(), &keymap(), &mut actions);
        let chords: Vec<(String, Option<u32>, Option<KeyInput>)> = top
            .iter()
            .flat_map(TopMenu::commands)
            .map(|c| {
                let chord = c.chord.clone();
                (
                    c.title.clone(),
                    chord.as_ref().map(|c| c.flags),
                    chord.map(|c| c.input),
                )
            })
            .collect();
        let text = |s: &str| Some(KeyInput::Text(s.into()));
        assert_eq!(
            chords,
            [
                ("New Shell".into(), Some(COMMAND), text("t")),
                ("Copy".into(), Some(COMMAND), text("c")),
                // Its only binding is in a context the default one lacks: shown all the same.
                ("Find…".into(), Some(COMMAND), text("f")),
                ("Quiet".into(), None, None),
                ("Commands…".into(), Some(COMMAND | SHIFT), text("p")),
                ("Other".into(), None, None),
                ("New Shell Again".into(), None, None),
            ]
        );
        let new_shell = top[0].commands()[0];
        assert_eq!(new_shell.keystroke, Some(ks("cmd-t")));
        let quiet = top[1].commands()[2];
        assert_eq!(quiet.keystroke, None, "no chord, no keystroke to deliver");
    }

    #[test]
    fn a_press_holds_exactly_the_chords_modifiers() {
        let palette = ks("cmd-shift-p");
        let held = |text: &str| ks(&format!("{text}-a")).modifiers;
        assert!(pressed(&palette, held("cmd-shift")));
        assert!(
            !pressed(&palette, Modifiers::default()),
            "a pick from the menu bar"
        );
        assert!(!pressed(&palette, held("cmd")));
        assert!(!pressed(&palette, held("ctrl-cmd-shift")));
    }
}
