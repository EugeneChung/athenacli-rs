//! Keybindings and edit modes, replacing Python `key_bindings.py`. Tab and
//! Ctrl-Space force the completion menu; F2/F3/F4 cannot mutate editor state
//! from inside reedline, so they exit `read_line` as `Signal::HostCommand`
//! sentinels (the typed buffer is suspended and restored) and the REPL loop
//! applies the toggle.

use reedline::{
    default_emacs_keybindings, default_vi_insert_keybindings, default_vi_normal_keybindings,
    EditMode, Emacs, KeyCode, KeyModifiers, Keybindings, ReedlineEvent, Vi,
};

/// Menu name shared by the completer menu registration and the Tab binding.
pub const COMPLETION_MENU: &str = "completion_menu";

/// `Signal::HostCommand` sentinels for the F-key toggles.
pub const TOGGLE_SMART_COMPLETION: &str = "toggle-smart-completion"; // F2
pub const TOGGLE_MULTI_LINE: &str = "toggle-multi-line"; // F3
pub const TOGGLE_EDIT_MODE: &str = "toggle-edit-mode"; // F4

/// Build the configured edit mode with our bindings layered on top of the
/// reedline defaults. Called again by the REPL whenever F4 flips `vi`.
pub fn edit_mode(vi: bool) -> Box<dyn EditMode> {
    if vi {
        let mut insert = default_vi_insert_keybindings();
        let mut normal = default_vi_normal_keybindings();
        add_athena_bindings(&mut insert);
        // Python binds the F-keys globally; mirror that in normal mode (menu
        // completion stays an insert-mode affair).
        add_toggle_bindings(&mut normal);
        Box::new(Vi::new(insert, normal))
    } else {
        let mut keybindings = default_emacs_keybindings();
        add_athena_bindings(&mut keybindings);
        Box::new(Emacs::new(keybindings))
    }
}

fn add_athena_bindings(keybindings: &mut Keybindings) {
    // Tab / Ctrl-Space: open the completion menu, or step to the next
    // candidate when it is already showing (Python `@kb.add('tab')` /
    // `@kb.add('c-space')`).
    let menu_or_next = ReedlineEvent::UntilFound(vec![
        ReedlineEvent::Menu(COMPLETION_MENU.to_string()),
        ReedlineEvent::MenuNext,
    ]);
    keybindings.add_binding(KeyModifiers::NONE, KeyCode::Tab, menu_or_next.clone());
    keybindings.add_binding(KeyModifiers::CONTROL, KeyCode::Char(' '), menu_or_next);
    add_toggle_bindings(keybindings);
}

fn add_toggle_bindings(keybindings: &mut Keybindings) {
    for (key, sentinel) in [
        (2, TOGGLE_SMART_COMPLETION),
        (3, TOGGLE_MULTI_LINE),
        (4, TOGGLE_EDIT_MODE),
    ] {
        keybindings.add_binding(
            KeyModifiers::NONE,
            KeyCode::F(key),
            ReedlineEvent::ExecuteHostCommand(sentinel.to_string()),
        );
    }
}
