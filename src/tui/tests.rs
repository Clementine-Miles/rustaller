use super::editor::{Editor, MAX_INPUT, TextValue};
use super::state::{Action, Authentication, EditSlot, Field, IsoMode, MenuFrame, Page, View};
use super::*;
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::{Terminal, backend::TestBackend};
use reinstall_rs::codec::encode_plan;
use reinstall_rs::config::{DnsMode, InstallProfile};
use reinstall_rs::host::args::{CliOutcome, parse_args};
use reinstall_rs::model::{BootMode, SecretString};
use reinstall_rs::postinstall::KernelChoice;

fn wizard() -> Wizard {
    Wizard::new(Some("https://assets.example/reinstall".into()))
}
fn press(wizard: &mut Wizard, code: KeyCode) {
    assert!(matches!(
        wizard.key(KeyEvent::new(code, KeyModifiers::NONE)),
        Progress::Continue
    ));
    if matches!(code, KeyCode::Esc | KeyCode::Left) && matches!(wizard.view, View::Leaving { .. }) {
        assert!(matches!(
            wizard.key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE)),
            Progress::Continue
        ));
    }
}
fn control(wizard: &mut Wizard, character: char) {
    assert!(matches!(
        wizard.key(KeyEvent::new(
            KeyCode::Char(character),
            KeyModifiers::CONTROL
        )),
        Progress::Continue
    ));
}
fn choose(wizard: &mut Wizard, index: usize) {
    wizard.frames.truncate(1);
    wizard.view = View::Menu;
    wizard.frames[0].selected = 0;
    press(wizard, KeyCode::Enter);
    wizard.frames[1].selected = index;
    press(wizard, KeyCode::Enter);
}
fn focus(wizard: &mut Wizard, predicate: impl Fn(&Action) -> bool) {
    let index = wizard
        .entries()
        .iter()
        .position(|e| predicate(&e.action))
        .expect("visible action");
    wizard.frames.last_mut().unwrap().selected = index;
}
fn open(wizard: &mut Wizard, page: Page) {
    let index = wizard
        .entries()
        .iter()
        .position(|entry| matches!(entry.action,Action::Open(target) if target==page))
        .expect("visible category");
    wizard.frames.last_mut().unwrap().selected = index;
    press(wizard, KeyCode::Enter);
}
fn set(wizard: &mut Wizard, field: Field, value: &str) {
    wizard.draft_mut().set(field, value.into());
}
fn credentials(wizard: &mut Wizard) {
    set(wizard, Field::Password, "test-password");
    set(wizard, Field::Confirm, "test-password");
}
fn render(wizard: &mut Wizard, width: u16, height: u16) -> String {
    let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
    terminal.draw(|frame| wizard.render(frame)).unwrap();
    // Wide glyphs occupy a leading cell and blank continuation cells. Snapshot
    // visible text rather than inserting artificial spaces inside CJK labels.
    let mut text = String::new();
    for row in terminal
        .backend()
        .buffer()
        .content
        .chunks(usize::from(width).max(1))
    {
        let mut column = 0;
        while column < row.len() {
            let symbol = row[column].symbol();
            text.push_str(symbol);
            column += ratatui::text::Line::raw(symbol).width().max(1);
        }
        text.push('\n');
    }
    text
}
fn assert_same_plan(wizard: &Wizard, args: &[&str], confhome: Option<&str>) {
    let CliOutcome::Plan(cli) = parse_args(args, confhome).unwrap() else {
        panic!("CLI plan")
    };
    assert_eq!(
        encode_plan(&wizard.build_plan().unwrap()).unwrap(),
        encode_plan(&cli).unwrap()
    );
}

#[test]
fn home_is_centered_english_and_selects_a_system_before_installation_settings() {
    let mut wizard = wizard();
    assert_eq!(wizard.page(), Page::Home);
    let screen = render(&mut wizard, 100, 30);
    assert!(screen.contains("RUSTALLER"));
    assert!(screen.contains("Install a system"));
    assert!(screen.contains("Configure current system"));
    assert!(
        !screen
            .chars()
            .any(|c| ('\u{4e00}'..='\u{9fff}').contains(&c))
    );
    let title = screen
        .lines()
        .find(|line| line.contains("RUSTALLER"))
        .unwrap();
    assert!(title.find("RUSTALLER").unwrap() > 20);
    press(&mut wizard, KeyCode::Enter);
    assert_eq!(wizard.page(), Page::Targets);
    assert!(
        wizard
            .entries()
            .iter()
            .any(|e| matches!(e.action, Action::Back))
    );
    press(&mut wizard, KeyCode::Down);
    press(&mut wizard, KeyCode::Enter);
    assert_eq!(wizard.choice, 1);
    assert_eq!(wizard.page(), Page::Overview);
    assert!(
        wizard
            .breadcrumb()
            .contains("Home / Choose a system / Debian")
    );
}

fn raw_key(wizard: &mut Wizard, key: KeyCode) {
    assert!(matches!(
        wizard.key(KeyEvent::new(key, KeyModifiers::NONE)),
        Progress::Continue
    ));
}

#[test]
fn explicit_back_offers_keep_discard_or_cancel_and_nested_snapshots_preserve_other_directories() {
    let mut wizard = wizard();
    choose(&mut wizard, 1);
    credentials(&mut wizard);
    open(&mut wizard, Page::Host);
    set(&mut wizard, Field::Hostname, "discard-me");
    focus(&mut wizard, |action| matches!(action, Action::Back));
    raw_key(&mut wizard, KeyCode::Enter);
    assert!(matches!(wizard.view, View::Leaving { selected: 0, .. }));
    let screen = render(&mut wizard, 100, 30);
    assert!(screen.contains("Save draft and go back"));
    assert!(screen.contains("Discard changes and go back"));
    assert!(screen.contains("Cancel - keep editing"));
    raw_key(&mut wizard, KeyCode::Esc);
    assert_eq!(wizard.page(), Page::Host);
    assert_eq!(wizard.draft().get(Field::Hostname), "discard-me");
    raw_key(&mut wizard, KeyCode::Esc);
    raw_key(&mut wizard, KeyCode::Down);
    raw_key(&mut wizard, KeyCode::Enter);
    assert_eq!(wizard.page(), Page::Overview);
    assert!(wizard.draft().get(Field::Hostname).is_empty());
    open(&mut wizard, Page::Host);
    set(&mut wizard, Field::Hostname, "saved-host");
    raw_key(&mut wizard, KeyCode::Esc);
    raw_key(&mut wizard, KeyCode::Enter);
    assert_eq!(wizard.page(), Page::Overview);
    assert_eq!(wizard.draft().get(Field::Hostname), "saved-host");
    open(&mut wizard, Page::Network);
    open(&mut wizard, Page::Dns);
    wizard.draft_mut().dns_mode = DnsMode::Static;
    set(&mut wizard, Field::DnsServers, "1.1.1.1");
    raw_key(&mut wizard, KeyCode::Esc);
    raw_key(&mut wizard, KeyCode::Enter);
    assert_eq!(wizard.page(), Page::Network);
    raw_key(&mut wizard, KeyCode::Esc);
    raw_key(&mut wizard, KeyCode::Down);
    raw_key(&mut wizard, KeyCode::Enter);
    assert_eq!(wizard.page(), Page::Overview);
    assert_eq!(wizard.draft().dns_mode, DnsMode::Auto);
    assert!(wizard.draft().get(Field::DnsServers).is_empty());
    assert_eq!(wizard.draft().get(Field::Hostname), "saved-host");
}

#[test]
fn current_system_route_has_no_installation_fields_and_builds_a_separate_noop_or_apply_request() {
    let mut wizard = wizard();
    wizard.current_target = Ok(reinstall_rs::model::Target::Debian);
    press(&mut wizard, KeyCode::Down);
    press(&mut wizard, KeyCode::Enter);
    assert!(wizard.is_current());
    assert_eq!(wizard.build_current().unwrap(), CurrentConfig::default());
    assert!(wizard.build_plan().is_err());
    assert!(!wizard.entries().iter().any(|e| matches!(
        e.action,
        Action::Open(Page::Boot | Page::Installation | Page::Resources | Page::Swap)
    )));
    open(&mut wizard, Page::Account);
    assert!(
        !wizard
            .entries()
            .iter()
            .any(|e| matches!(e.action, Action::Open(Page::Auth | Page::Passwords)))
    );
    press(&mut wizard, KeyCode::Esc);
    open(&mut wizard, Page::Host);
    assert!(
        !wizard
            .entries()
            .iter()
            .any(|e| matches!(e.action, Action::Select(super::state::Setting::Kernel)))
    );
    set(&mut wizard, Field::Hostname, "current-host");
    press(&mut wizard, KeyCode::Esc);
    press(&mut wizard, KeyCode::F(2));
    let Progress::ConfigureCurrent(config) =
        wizard.key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE))
    else {
        panic!("separate current-system request")
    };
    assert_eq!(config.hostname.as_deref(), Some("current-host"));
    assert!(config.ssh_port.is_none());
    assert!(config.ssh_keys.is_empty());
    assert!(!wizard.summary().join("\n").contains("Target disk"));
    wizard.view = View::Menu;
    wizard
        .draft_mut()
        .keys
        .push(TextValue::new("https://remote.example/keys"));
    assert!(wizard.build_current().is_err());
    choose(&mut wizard, 1);
    assert!(!wizard.is_current());
    assert!(wizard.draft().get(Field::Hostname).is_empty());
}

#[test]
fn returning_from_current_mode_to_system_selection_preserves_its_draft_without_reusing_its_route() {
    let mut wizard = wizard();
    wizard.current_target = Ok(reinstall_rs::model::Target::Debian);
    press(&mut wizard, KeyCode::Down);
    press(&mut wizard, KeyCode::Enter);
    set(&mut wizard, Field::Hostname, "current-session-host");
    raw_key(&mut wizard, KeyCode::Esc);
    assert!(matches!(wizard.view, View::Leaving { .. }));
    raw_key(&mut wizard, KeyCode::Enter);
    assert_eq!(wizard.page(), Page::Home);
    press(&mut wizard, KeyCode::Home);
    press(&mut wizard, KeyCode::Enter);
    assert_eq!(wizard.page(), Page::Targets);
    assert!(!wizard.is_current());
    assert!(wizard.draft().get(Field::Hostname).is_empty());
    press(&mut wizard, KeyCode::Esc);
    press(&mut wizard, KeyCode::Down);
    press(&mut wizard, KeyCode::Enter);
    assert!(wizard.is_current());
    assert_eq!(wizard.draft().get(Field::Hostname), "current-session-host");
}

#[test]
fn successful_current_apply_shows_backups_and_updates_the_saved_directory_baselines() {
    use reinstall_rs::host::current::CurrentReport;
    let mut wizard = wizard();
    wizard.current_target = Ok(reinstall_rs::model::Target::Debian);
    press(&mut wizard, KeyCode::Down);
    press(&mut wizard, KeyCode::Enter);
    open(&mut wizard, Page::Host);
    set(&mut wizard, Field::Hostname, "applied-host");
    wizard.complete_current(Ok(CurrentReport {
        changed_files: vec!["etc/hostname".into()],
        backup_dir: Some("/var/backups/rust-aller-current/example".into()),
        notes: vec!["Network and SSH were not restarted.".into()],
    }));
    let View::Outcome {
        message, success, ..
    } = &wizard.view
    else {
        panic!("result")
    };
    assert!(*success);
    assert!(message.contains("etc/hostname"));
    assert!(message.contains("/var/backups/rust-aller-current/example"));
    press(&mut wizard, KeyCode::Enter);
    press(&mut wizard, KeyCode::Esc);
    raw_key(&mut wizard, KeyCode::Esc);
    assert!(matches!(wizard.view, View::Menu));
    assert_eq!(wizard.page(), Page::Overview);
    open(&mut wizard, Page::Host);
    set(&mut wizard, Field::Hostname, "unsaved-replacement");
    raw_key(&mut wizard, KeyCode::Esc);
    raw_key(&mut wizard, KeyCode::Down);
    raw_key(&mut wizard, KeyCode::Enter);
    assert_eq!(wizard.draft().get(Field::Hostname), "applied-host");
}

#[test]
fn current_apply_result_redacts_key_sources_and_cancel_never_submits_a_configuration() {
    use reinstall_rs::host::current::CurrentReport;
    let mut wizard = wizard();
    wizard.current_target = Ok(reinstall_rs::model::Target::Debian);
    press(&mut wizard, KeyCode::Down);
    press(&mut wizard, KeyCode::Enter);
    open(&mut wizard, Page::Account);
    open(&mut wizard, Page::Keys);
    wizard
        .draft_mut()
        .keys
        .push(TextValue::new("/root/PUBLIC-KEY-SOURCE-SENTINEL"));
    wizard.complete_current(Ok(CurrentReport {
        changed_files: vec!["root/.ssh/authorized_keys".into()],
        backup_dir: None,
        notes: vec!["/root/PUBLIC-KEY-SOURCE-SENTINEL\u{1b}[31m".into()],
    }));
    let View::Outcome { message, .. } = &wizard.view else {
        panic!("result")
    };
    assert!(!message.contains("PUBLIC-KEY-SOURCE-SENTINEL"));
    assert!(!message.contains('\u{1b}'));
    press(&mut wizard, KeyCode::Enter);
    press(&mut wizard, KeyCode::Esc);
    set(&mut wizard, Field::Hostname, "unapplied-draft");
    raw_key(&mut wizard, KeyCode::Esc);
    assert!(matches!(wizard.view, View::Leaving { .. }));
    raw_key(&mut wizard, KeyCode::Down);
    raw_key(&mut wizard, KeyCode::Down);
    raw_key(&mut wizard, KeyCode::Enter);
    assert!(matches!(wizard.view, View::Menu));
    assert_eq!(wizard.page(), Page::Keys);
    assert!(matches!(
        wizard.key(KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL)),
        Progress::Cancel
    ));
}

#[test]
fn keyboard_navigates_nested_categories_and_editor_commit_or_cancel_preserves_draft() {
    let mut wizard = wizard();
    press(&mut wizard, KeyCode::Enter); // installation path
    press(&mut wizard, KeyCode::Enter); // Alpine
    press(&mut wizard, KeyCode::Enter); // account
    assert_eq!(wizard.page(), Page::Account);
    press(&mut wizard, KeyCode::Enter); // username
    control(&mut wizard, 'u');
    wizard.paste("discarded");
    press(&mut wizard, KeyCode::Esc);
    assert_eq!(wizard.draft().get(Field::Username), "root");
    press(&mut wizard, KeyCode::Enter);
    control(&mut wizard, 'u');
    wizard.paste("alice");
    press(&mut wizard, KeyCode::Enter);
    assert_eq!(wizard.draft().get(Field::Username), "alice");
    press(&mut wizard, KeyCode::Down);
    press(&mut wizard, KeyCode::Enter);
    assert_eq!(wizard.page(), Page::Auth);
    press(&mut wizard, KeyCode::Down);
    press(&mut wizard, KeyCode::Enter);
    assert_eq!(wizard.page(), Page::Passwords);
    assert!(
        wizard
            .breadcrumb()
            .contains("Account and SSH / Authentication / Password settings")
    );
    press(&mut wizard, KeyCode::Enter);
    wizard.paste("  exact password  ");
    press(&mut wizard, KeyCode::Enter);
    press(&mut wizard, KeyCode::Down);
    press(&mut wizard, KeyCode::Enter);
    wizard.paste("  exact password  ");
    press(&mut wizard, KeyCode::Enter);
    for _ in 0..3 {
        press(&mut wizard, KeyCode::Esc);
    }
    assert_eq!(wizard.page(), Page::Overview);
    assert_eq!(
        wizard
            .build_plan()
            .unwrap()
            .password
            .as_ref()
            .map(SecretString::expose),
        Some("  exact password  ")
    );
}

#[test]
fn password_confirmation_and_selected_authentication_are_required_without_secret_errors() {
    let mut wizard = wizard();
    choose(&mut wizard, 1);
    assert!(wizard.build_plan().is_err());
    set(&mut wizard, Field::Password, "UNIQUE-PASSWORD");
    set(&mut wizard, Field::Confirm, "DIFFERENT-SECRET");
    let error = wizard.build_plan().err().unwrap();
    assert!(error.contains("do not match"));
    assert!(!error.contains("SECRET"));
    assert!(!error.contains("PASSWORD"));
    set(&mut wizard, Field::Confirm, "UNIQUE-PASSWORD");
    wizard
        .draft_mut()
        .keys
        .push(TextValue::new("ssh-ed25519 AAAA hidden@example"));
    let plan = wizard.build_plan().unwrap();
    assert!(plan.password.is_some());
    assert!(plan.ssh_keys.is_empty());
    wizard.draft_mut().authentication = Authentication::PublicKeys;
    let plan = wizard.build_plan().unwrap();
    assert!(plan.password.is_none());
    assert_eq!(plan.ssh_keys.len(), 1);
    wizard.draft_mut().keys.clear();
    assert!(wizard.build_plan().is_err());
}

#[test]
fn public_key_manager_adds_edits_removes_and_switches_authentication_with_keyboard() {
    let mut wizard = wizard();
    choose(&mut wizard, 0);
    credentials(&mut wizard);
    open(&mut wizard, Page::Account);
    open(&mut wizard, Page::Auth);
    press(&mut wizard, KeyCode::Enter); // method selector
    press(&mut wizard, KeyCode::Down);
    press(&mut wizard, KeyCode::Enter);
    open(&mut wizard, Page::Keys);
    focus(&mut wizard, |action| {
        matches!(action, Action::Edit(EditSlot::NewKey))
    });
    press(&mut wizard, KeyCode::Enter);
    wizard.paste("ssh-ed25519 AAAA first@host");
    press(&mut wizard, KeyCode::Enter);
    focus(&mut wizard, |action| {
        matches!(action, Action::Edit(EditSlot::NewKey))
    });
    press(&mut wizard, KeyCode::Enter);
    wizard.paste("github:alice");
    press(&mut wizard, KeyCode::Enter);
    assert_eq!(wizard.draft().keys.len(), 2);
    press(&mut wizard, KeyCode::Home);
    press(&mut wizard, KeyCode::Enter);
    press(&mut wizard, KeyCode::Enter);
    control(&mut wizard, 'u');
    wizard.paste("/root/keys.pub");
    press(&mut wizard, KeyCode::Enter);
    assert_eq!(wizard.draft().keys[0].0, "/root/keys.pub");
    press(&mut wizard, KeyCode::Down);
    press(&mut wizard, KeyCode::Enter);
    assert_eq!(wizard.page(), Page::Keys);
    assert_eq!(wizard.draft().keys.len(), 1);
    assert_eq!(wizard.build_plan().unwrap().ssh_keys, ["github:alice"]);
    press(&mut wizard, KeyCode::Esc);
    press(&mut wizard, KeyCode::Home);
    press(&mut wizard, KeyCode::Enter);
    press(&mut wizard, KeyCode::Up);
    press(&mut wizard, KeyCode::Enter);
    assert!(wizard.draft().authentication == Authentication::Password);
    assert!(wizard.build_plan().unwrap().ssh_keys.is_empty());
}

#[test]
fn private_key_input_and_overfull_key_list_are_rejected_without_echoing() {
    let mut wizard = wizard();
    choose(&mut wizard, 0);
    open(&mut wizard, Page::Account);
    open(&mut wizard, Page::Auth);
    wizard.draft_mut().authentication = Authentication::PublicKeys;
    open(&mut wizard, Page::Keys);
    focus(&mut wizard, |action| {
        matches!(action, Action::Edit(EditSlot::NewKey))
    });
    press(&mut wizard, KeyCode::Enter);
    wizard.paste("-----BEGIN OPENSSH PRIVATE KEY----- UNIQUE-KEY-SECRET");
    press(&mut wizard, KeyCode::Enter);
    assert!(matches!(wizard.view, View::Edit { .. }));
    assert!(wizard.draft().keys.is_empty());
    assert!(!render(&mut wizard, 100, 28).contains("UNIQUE-KEY-SECRET"));
    press(&mut wizard, KeyCode::Esc);
    wizard.draft_mut().keys = (0..129).map(|_| TextValue::new("github:alice")).collect();
    assert!(wizard.build_plan().is_err());
}

#[test]
fn tcp_and_swap_submenus_reuse_cli_validation_and_inactive_swap_values_are_ignored() {
    for (index, target) in [(0, "alpine"), (1, "debian"), (2, "arch"), (3, "gentoo")] {
        let mut wizard = wizard();
        choose(&mut wizard, index);
        credentials(&mut wizard);
        set(&mut wizard, Field::SwapSize, "invalid inactive size");
        set(&mut wizard, Field::Swappiness, "invalid inactive value");
        assert!(wizard.build_plan().unwrap().system.swap.size_mib.is_none());
        open(&mut wizard, Page::Network);
        open(&mut wizard, Page::Tcp);
        wizard.frames.last_mut().unwrap().selected = wizard
            .entries()
            .iter()
            .position(|e| matches!(e.action, Action::TcpPreset))
            .unwrap();
        press(&mut wizard, KeyCode::Enter);
        press(&mut wizard, KeyCode::Esc);
        press(&mut wizard, KeyCode::Esc);
        open(&mut wizard, Page::Swap);
        press(&mut wizard, KeyCode::Enter);
        set(&mut wizard, Field::SwapSize, "2048");
        set(&mut wizard, Field::Swappiness, "10");
        assert_same_plan(
            &wizard,
            &[
                target,
                "--username=root",
                "--password=test-password",
                "--ssh-port=22",
                "--web-port=80",
                "--tcp-congestion=bbr",
                "--qdisc=fq",
                "--tcp-buffer-mib=64",
                "--somaxconn=8192",
                "--syn-backlog=8192",
                "--netdev-backlog=16384",
                "--tcp-mtu-probing=1",
                "--tcp-fastopen=3",
                "--tcp-syncookies=1",
                "--swapfile-mib=2048",
                "--swappiness=10",
            ],
            Some("https://assets.example/reinstall"),
        );
        assert!(wizard.summary().join("\n").contains("2048 MiB"));
        set(&mut wizard, Field::SwapSize, "0");
        assert!(wizard.build_plan().is_err());
        focus(&mut wizard, |action| matches!(action, Action::ToggleSwap));
        press(&mut wizard, KeyCode::Enter);
        assert!(wizard.build_plan().unwrap().system.swap.size_mib.is_none());
    }
}

#[test]
fn swap_size_and_new_hostname_are_edited_saved_and_submitted_per_target() {
    let mut wizard = wizard();
    choose(&mut wizard, 0);
    credentials(&mut wizard);
    open(&mut wizard, Page::Host);
    focus(&mut wizard, |action| {
        matches!(action, Action::Edit(EditSlot::Field(Field::Hostname)))
    });
    press(&mut wizard, KeyCode::Enter);
    wizard.paste("alpine-node");
    press(&mut wizard, KeyCode::Enter);
    press(&mut wizard, KeyCode::Esc);
    open(&mut wizard, Page::Swap);
    press(&mut wizard, KeyCode::Enter);
    assert!(matches!(
        wizard.entries()[wizard.frames.last().unwrap().selected].action,
        Action::Edit(EditSlot::Field(Field::SwapSize))
    ));
    press(&mut wizard, KeyCode::Enter);
    wizard.key(KeyEvent::new(KeyCode::Char('u'), KeyModifiers::CONTROL));
    wizard.paste("4096");
    press(&mut wizard, KeyCode::Enter);
    focus(&mut wizard, |action| matches!(action, Action::ToggleSwap));
    press(&mut wizard, KeyCode::Enter);
    assert!(wizard.build_plan().unwrap().system.swap.size_mib.is_none());
    assert_eq!(wizard.draft().get(Field::SwapSize), "4096");
    press(&mut wizard, KeyCode::Enter);
    press(&mut wizard, KeyCode::Esc);
    let plan = wizard.build_plan().unwrap();
    assert_eq!(plan.system.swap.size_mib, Some(4096));
    assert_eq!(plan.post_install.hostname.as_deref(), Some("alpine-node"));
    assert!(wizard.summary().join("\n").contains("4096 MiB"));
    choose(&mut wizard, 1);
    assert!(wizard.draft().get(Field::Hostname).is_empty());
    assert!(!wizard.draft().swap_enabled);
    choose(&mut wizard, 0);
    assert_eq!(
        wizard.build_plan().unwrap().system.swap.size_mib,
        Some(4096)
    );
    assert_eq!(
        wizard
            .build_plan()
            .unwrap()
            .post_install
            .hostname
            .as_deref(),
        Some("alpine-node")
    );
}

#[test]
fn native_key_generator_is_nested_and_only_generated_public_key_enters_the_plan() {
    use reinstall_rs::host::ssh_keygen::GeneratedKey;
    let mut wizard = wizard();
    choose(&mut wizard, 1);
    wizard.draft_mut().authentication = Authentication::PublicKeys;
    open(&mut wizard, Page::Account);
    open(&mut wizard, Page::Auth);
    open(&mut wizard, Page::Keys);
    open(&mut wizard, Page::Keygen);
    set(
        &mut wizard,
        Field::KeyPath,
        "/tmp/rustaller-keygen-nonexistent-unit-path/id_unique",
    );
    set(&mut wizard, Field::KeyComment, "20261026");
    focus(&mut wizard, |action| matches!(action, Action::GenerateKey));
    let Progress::GenerateKey(options) =
        wizard.key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE))
    else {
        panic!("native tool action")
    };
    assert_eq!(options.rounds, 512);
    assert_eq!(options.cipher, "aes256-ctr");
    assert_eq!(options.key_type.as_str(), "ed25519");
    assert_eq!(options.comment, "20261026");
    wizard.keygen_complete(Ok(GeneratedKey {
        public_key: "ssh-ed25519 AAAA generated-key".into(),
        private_path: options.output.clone(),
        public_path: options.output.with_extension("pub"),
    }));
    let plan = wizard.build_plan().unwrap();
    assert!(plan.password.is_none());
    assert_eq!(plan.ssh_keys, ["ssh-ed25519 AAAA generated-key"]);
    let bytes = encode_plan(&plan).unwrap();
    assert!(!bytes.windows(b"id_unique".len()).any(|v| v == b"id_unique"));
    let screen = render(&mut wizard, 100, 30);
    assert!(screen.contains("Back up"));
    press(&mut wizard, KeyCode::Enter);
    assert!(matches!(wizard.view, View::Menu));
    assert_eq!(wizard.draft().keys.len(), 1);
    set(&mut wizard, Field::KeyRounds, "0");
    press(&mut wizard, KeyCode::Enter);
    assert!(!wizard.error.is_empty());
}

#[test]
fn text_editor_handles_unicode_cursor_delete_and_atomic_input_limits() {
    let mut input = Editor::new("甲ab", false);
    input
        .key(KeyEvent::new(KeyCode::Home, KeyModifiers::NONE))
        .unwrap();
    input
        .key(KeyEvent::new(KeyCode::Right, KeyModifiers::NONE))
        .unwrap();
    input.paste("乙").unwrap();
    assert_eq!(input.value.0, "甲乙ab");
    input
        .key(KeyEvent::new(KeyCode::Delete, KeyModifiers::NONE))
        .unwrap();
    assert_eq!(input.value.0, "甲乙b");
    input
        .key(KeyEvent::new(KeyCode::Backspace, KeyModifiers::NONE))
        .unwrap();
    assert_eq!(input.value.0, "甲b");
    input
        .key(KeyEvent::new(KeyCode::End, KeyModifiers::NONE))
        .unwrap();
    assert_eq!(input.cursor, 2);
    let before = input.value.0.clone();
    let cursor = input.cursor;
    assert!(input.paste(&"x".repeat(MAX_INPUT)).is_err());
    assert_eq!(input.value.0, before);
    assert_eq!(input.cursor, cursor);
    assert!(input.paste("\u{1b}[31m\nsecret").is_err());
    assert_eq!(input.value.0, before);
    let mut secret = Editor::new(&"x".repeat(MAX_INPUT - 1), true);
    assert!(secret.paste("你").is_err());
    assert_eq!(secret.value.0.len(), MAX_INPUT - 1);
    secret.paste("y").unwrap();
    assert!(
        secret
            .key(KeyEvent::new(KeyCode::Char('z'), KeyModifiers::NONE))
            .is_err()
    );
    let (shown, cursor) = secret.visible(20);
    assert!(shown.chars().all(|character| character == '*'));
    assert!(shown.len() <= 20);
    assert!(cursor < 20);
}

#[test]
fn overflow_is_visible_and_cannot_commit_a_partial_password() {
    let mut wizard = wizard();
    choose(&mut wizard, 0);
    open(&mut wizard, Page::Account);
    open(&mut wizard, Page::Auth);
    open(&mut wizard, Page::Passwords);
    press(&mut wizard, KeyCode::Enter);
    wizard.paste("original");
    wizard.paste(&"x".repeat(MAX_INPUT));
    assert!(wizard.error.contains("8192"));
    press(&mut wizard, KeyCode::Enter);
    assert_eq!(wizard.draft().get(Field::Password), "original");
}

#[test]
fn drafts_survive_back_navigation_and_switching_targets_without_cross_target_options() {
    let mut wizard = wizard();
    choose(&mut wizard, 1);
    credentials(&mut wizard);
    set(&mut wizard, Field::Username, "debian_user");
    set(&mut wizard, Field::Hostname, "debian-vps");
    wizard.draft_mut().kernel = KernelChoice::Generic;
    choose(&mut wizard, 2);
    credentials(&mut wizard);
    wizard.draft_mut().kernel = KernelChoice::Lts;
    set(&mut wizard, Field::Username, "arch_user");
    choose(&mut wizard, 1);
    assert_eq!(wizard.draft().get(Field::Username), "debian_user");
    assert!(wizard.draft().kernel == KernelChoice::Generic);
    assert_eq!(
        wizard
            .build_plan()
            .unwrap()
            .post_install
            .hostname
            .as_deref(),
        Some("debian-vps")
    );
    choose(&mut wizard, 2);
    assert_eq!(wizard.draft().get(Field::Username), "arch_user");
    assert!(wizard.draft().kernel == KernelChoice::Lts);
    choose(&mut wizard, 4);
    set(&mut wizard, Field::IsoPath, "/images/live.iso");
    let plan = wizard.build_plan().unwrap();
    assert!(plan.password.is_none());
    assert!(plan.username.is_none());
    assert!(plan.post_install.is_default());
}

#[test]
fn all_automatic_targets_match_cli_plan_encoding_including_multiple_keys_and_host_fields() {
    for (index, target, kernel) in [
        (0, "alpine", "cloud"),
        (1, "debian", "generic"),
        (2, "arch", "lts"),
        (3, "gentoo", "standard"),
    ] {
        let mut wizard = wizard();
        choose(&mut wizard, index);
        set(&mut wizard, Field::Username, "admin");
        set(&mut wizard, Field::Hostname, "my-vps");
        set(&mut wizard, Field::Timezone, "Etc/UTC");
        set(&mut wizard, Field::Disk, "/dev/vda");
        set(&mut wizard, Field::SshPort, "2200");
        set(&mut wizard, Field::WebPort, "8080");
        set(&mut wizard, Field::Frpc, "/root/frpc.toml");
        wizard.draft_mut().kernel = kernel.parse().unwrap();
        wizard.draft_mut().boot = Some(BootMode::Efi);
        wizard.draft_mut().hold = Some(2);
        wizard.draft_mut().debug = true;
        wizard.draft_mut().authentication = Authentication::PublicKeys;
        wizard.draft_mut().keys = vec![
            TextValue::new("ssh-ed25519 AAAA one@host"),
            TextValue::new("github:alice"),
        ];
        assert_same_plan(
            &wizard,
            &[
                target,
                "--username",
                "admin",
                "--ssh-port",
                "2200",
                "--web-port",
                "8080",
                "--target-disk",
                "/dev/vda",
                "--frpc-config",
                "/root/frpc.toml",
                "--kernel",
                kernel,
                "--hostname",
                "my-vps",
                "--timezone",
                "Etc/UTC",
                "--force-boot-mode",
                "efi",
                "--hold",
                "2",
                "--debug",
                "--ssh-key",
                "ssh-ed25519 AAAA one@host",
                "--ssh-key",
                "github:alice",
            ],
            Some("https://assets.example/reinstall"),
        );
    }
}

#[test]
fn rescue_and_manual_targets_match_cli_and_hide_installation_categories() {
    let mut wizard = wizard();
    choose(&mut wizard, 5);
    credentials(&mut wizard);
    wizard.draft_mut().generic_kernel = true;
    assert_same_plan(
        &wizard,
        &[
            "alpine",
            "--username",
            "root",
            "--ssh-port",
            "22",
            "--web-port",
            "80",
            "--password",
            "test-password",
            "--hold",
            "1",
            "--no-cloud-kernel",
        ],
        Some("https://assets.example/reinstall"),
    );
    assert!(
        !wizard
            .entries()
            .iter()
            .any(|entry| matches!(entry.action, Action::Open(Page::Host)))
    );
    open(&mut wizard, Page::Boot);
    assert!(!wizard.entries().iter().any(|entry| matches!(
        entry.action,
        Action::Edit(EditSlot::Field(Field::Disk)) | Action::Select(super::state::Setting::Hold)
    )));
    choose(&mut wizard, 6);
    wizard.draft_mut().boot = Some(BootMode::Bios);
    assert_same_plan(&wizard, &["netboot.xyz", "--force-boot-mode", "bios"], None);
    choose(&mut wizard, 7);
    assert_same_plan(&wizard, &["reset"], None);
    press(&mut wizard, KeyCode::Enter);
    assert_same_plan(&wizard, &["reset", "--debug"], None);
    assert!(wizard.summary().join("\n").contains("Debug output: On"));
    for index in [4, 6, 7] {
        choose(&mut wizard, index);
        assert!(!wizard.entries().iter().any(|entry| matches!(
            entry.action,
            Action::Open(Page::Account | Page::Host | Page::Resources)
        )));
    }
}

#[test]
fn iso_modes_use_only_active_fields_and_validate_pure_grub_safety() {
    let mut wizard = wizard();
    choose(&mut wizard, 4);
    set(&mut wizard, Field::IsoPath, "/images/live.iso");
    set(&mut wizard, Field::IsoKernel, "/live/vmlinuz");
    set(&mut wizard, Field::IsoInitrd, "/live/initrd.img");
    assert_same_plan(&wizard, &["iso", "--iso", "/images/live.iso"], None);
    wizard.draft_mut().iso_mode = IsoMode::Kernel;
    set(&mut wizard, Field::IsoConfig, "/boot/grub/loopback.cfg");
    set(
        &mut wizard,
        Field::IsoCmdline,
        "boot=live findiso=%ISO_PATH%",
    );
    assert_same_plan(
        &wizard,
        &[
            "iso",
            "--iso",
            "/images/live.iso",
            "--iso-kernel",
            "/live/vmlinuz",
            "--iso-initrd",
            "/live/initrd.img",
            "--iso-cmdline",
            "boot=live findiso=%ISO_PATH%",
        ],
        None,
    );
    set(&mut wizard, Field::IsoPath, "relative.iso");
    assert!(wizard.build_plan().is_err());
    set(&mut wizard, Field::IsoPath, "/images/live.iso");
    set(&mut wizard, Field::IsoCmdline, "boot=live;reboot");
    assert!(wizard.build_plan().is_err());
    set(&mut wizard, Field::IsoCmdline, "");
    set(&mut wizard, Field::IsoInitrd, "");
    assert!(wizard.build_plan().is_err());
    set(&mut wizard, Field::IsoKernel, "");
    assert!(wizard.build_plan().is_err());
}

#[test]
fn selectors_offer_only_target_kernels_and_do_not_apply_cancelled_choices() {
    let mut wizard = wizard();
    choose(&mut wizard, 1);
    credentials(&mut wizard);
    open(&mut wizard, Page::Host);
    focus(&mut wizard, |action| {
        matches!(action, Action::Select(super::state::Setting::Kernel))
    });
    press(&mut wizard, KeyCode::Enter);
    let View::Select { setting, .. } = wizard.view else {
        panic!("selector")
    };
    assert_eq!(wizard.choices(setting), ["auto", "cloud", "generic"]);
    press(&mut wizard, KeyCode::End);
    press(&mut wizard, KeyCode::Esc);
    assert!(wizard.draft().kernel == KernelChoice::Auto);
    press(&mut wizard, KeyCode::Enter);
    press(&mut wizard, KeyCode::End);
    press(&mut wizard, KeyCode::Enter);
    assert!(wizard.draft().kernel == KernelChoice::Generic);
}

#[test]
fn validation_failures_and_all_sensitive_views_hide_raw_values() {
    let mut wizard = wizard();
    choose(&mut wizard, 1);
    credentials(&mut wizard);
    set(&mut wizard, Field::Password, "UNIQUE-PASSWORD");
    set(&mut wizard, Field::Confirm, "UNIQUE-PASSWORD");
    set(
        &mut wizard,
        Field::Frpc,
        "https://example.org/UNIQUE-FRPC-TOKEN",
    );
    set(
        &mut wizard,
        Field::Confhome,
        "https://user:UNIQUE-URL-TOKEN@example.org/path?token=secret",
    );
    open(&mut wizard, Page::Resources);
    press(&mut wizard, KeyCode::Enter);
    assert!(!render(&mut wizard, 100, 28).contains("UNIQUE-URL-TOKEN"));
    press(&mut wizard, KeyCode::Esc);
    press(&mut wizard, KeyCode::F(2));
    let screen = render(&mut wizard, 100, 28);
    assert!(!screen.contains("UNIQUE-"));
    assert!(!wizard.error.contains("UNIQUE-"));
    set(
        &mut wizard,
        Field::Confhome,
        "https://assets.example/reinstall",
    );
    press(&mut wizard, KeyCode::F(2));
    let screen = render(&mut wizard, 100, 38);
    assert!(!screen.contains("UNIQUE-"));
    let summary = wizard.summary().join("\n");
    assert!(summary.contains("Kernel"));
    assert!(summary.contains("Boot mode"));
    assert!(summary.contains("Web log port"));
    assert!(!summary.contains("UNIQUE-"));
}

#[test]
fn username_and_option_like_values_cannot_hijack_shared_parser() {
    let mut wizard = wizard();
    choose(&mut wizard, 0);
    credentials(&mut wizard);
    for username in ["--help", "-root", "a b", "bad\nname", ""] {
        set(&mut wizard, Field::Username, username);
        assert!(wizard.build_plan().is_err());
    }
    set(&mut wizard, Field::Username, "admin");
    set(&mut wizard, Field::Frpc, "--help");
    let plan = wizard.build_plan().unwrap();
    assert_eq!(plan.frpc_config.as_deref(), Some("--help"));
    set(&mut wizard, Field::Confhome, "--help");
    assert!(wizard.build_plan().is_err());
}

#[test]
fn summary_displays_effective_trimmed_options_but_keeps_exact_password_whitespace() {
    let mut wizard = wizard();
    choose(&mut wizard, 1);
    set(&mut wizard, Field::Username, "  alice  ");
    set(&mut wizard, Field::Hostname, "  my-vps  ");
    set(
        &mut wizard,
        Field::Confhome,
        "  https://assets.example/reinstall  ",
    );
    set(&mut wizard, Field::Frpc, "   ");
    set(&mut wizard, Field::Password, "  exact  ");
    set(&mut wizard, Field::Confirm, "  exact  ");
    let plan = wizard.build_plan().unwrap();
    assert_eq!(plan.username.as_deref(), Some("alice"));
    assert!(plan.frpc_config.is_none());
    assert_eq!(
        plan.password.as_ref().map(SecretString::expose),
        Some("  exact  ")
    );
    let summary = wizard.summary().join("\n");
    assert!(summary.contains("Username: alice"));
    assert!(summary.contains("Hostname: my-vps"));
    assert!(summary.contains("https://assets.example/…"));
    assert!(summary.contains("frpc: Not set"));
    choose(&mut wizard, 4);
    wizard.draft_mut().iso_mode = IsoMode::Kernel;
    set(&mut wizard, Field::IsoPath, " /images/live.iso ");
    set(&mut wizard, Field::IsoKernel, " /live/vmlinuz ");
    set(&mut wizard, Field::IsoInitrd, " /live/initrd ");
    set(&mut wizard, Field::IsoCmdline, "   ");
    assert!(
        wizard
            .build_plan()
            .unwrap()
            .iso
            .as_ref()
            .unwrap()
            .cmdline
            .is_none()
    );
    assert!(
        wizard
            .summary()
            .join("\n")
            .contains("ISO kernel command line: Not set")
    );
}

#[test]
fn result_screen_redacts_effective_values_and_returns_without_auto_retry() {
    let mut wizard = wizard();
    choose(&mut wizard, 1);
    credentials(&mut wizard);
    set(
        &mut wizard,
        Field::Frpc,
        "  https://private.example/FRPC-TOKEN  ",
    );
    let plan = wizard.build_plan().unwrap();
    let mut calls = 0;
    execute_ready(
        &mut wizard,
        &mut |_| {
            calls += 1;
            Err("first line\nhttps://private.example/FRPC-TOKEN test-password\u{1b}[31m".into())
        },
        &plan,
    );
    assert_eq!(calls, 1);
    let View::Outcome { message, .. } = &wizard.view else {
        panic!("outcome")
    };
    assert!(message.contains('\n'));
    assert!(!message.contains("FRPC-TOKEN"));
    assert!(!message.contains("test-password"));
    assert!(!message.contains('\u{1b}'));
    press(&mut wizard, KeyCode::Enter);
    assert!(matches!(wizard.view, View::Review { .. }));
    assert_eq!(calls, 1);
    press(&mut wizard, KeyCode::Esc);
    assert!(matches!(wizard.view, View::Menu));
    assert_eq!(wizard.draft().get(Field::Password), "test-password");
}

#[test]
fn lists_and_review_scroll_and_tiny_terminal_blocks_hidden_actions() {
    let mut wizard = wizard();
    choose(&mut wizard, 0);
    credentials(&mut wizard);
    wizard.draft_mut().authentication = Authentication::PublicKeys;
    wizard.draft_mut().keys = (0..30)
        .map(|index| TextValue::new(format!("github:user{index}")))
        .collect();
    wizard.frames.push(MenuFrame {
        page: Page::Keys,
        selected: 0,
        offset: 0,
        baseline: None,
    });
    press(&mut wizard, KeyCode::End);
    let screen = render(&mut wizard, 55, 18);
    assert!(screen.contains("Add public key"));
    assert!(wizard.frames.last().unwrap().offset > 0);
    press(&mut wizard, KeyCode::F(2));
    press(&mut wizard, KeyCode::End);
    let screen = render(&mut wizard, 55, 18);
    assert!(screen.contains("BIOS/UEFI"));
    assert!(matches!(wizard.view,View::Review {scroll} if scroll>0));
    render(&mut wizard, 12, 4);
    assert!(matches!(
        wizard.key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE)),
        Progress::Continue
    ));
    assert!(matches!(wizard.view, View::Review { .. }));
    render(&mut wizard, 80, 30);
    press(&mut wizard, KeyCode::Home);
    render(&mut wizard, 80, 30);
    assert!(matches!(wizard.view, View::Review { scroll: 0 }));
    assert!(matches!(
        wizard.key(KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL)),
        Progress::Cancel
    ));
}

#[test]
fn minimal_and_dns_submenus_match_cli_for_all_four_targets() {
    for (index, target) in [(0, "alpine"), (1, "debian"), (2, "arch"), (3, "gentoo")] {
        let mut wizard = wizard();
        choose(&mut wizard, index);
        credentials(&mut wizard);
        open(&mut wizard, Page::Installation);
        press(&mut wizard, KeyCode::Enter);
        press(&mut wizard, KeyCode::Down);
        press(&mut wizard, KeyCode::Enter);
        assert_eq!(wizard.draft().profile, InstallProfile::Minimal);
        for _ in 0..3 {
            press(&mut wizard, KeyCode::Down);
            press(&mut wizard, KeyCode::Enter);
        }
        press(&mut wizard, KeyCode::Esc);
        open(&mut wizard, Page::Network);
        open(&mut wizard, Page::Dns);
        press(&mut wizard, KeyCode::Enter);
        press(&mut wizard, KeyCode::Down);
        press(&mut wizard, KeyCode::Enter);
        press(&mut wizard, KeyCode::Down);
        press(&mut wizard, KeyCode::Enter);
        wizard.paste("1.1.1.1,2606:4700:4700::1111");
        press(&mut wizard, KeyCode::Enter);
        press(&mut wizard, KeyCode::Down);
        press(&mut wizard, KeyCode::Enter);
        wizard.paste("example.internal");
        press(&mut wizard, KeyCode::Enter);
        assert_same_plan(
            &wizard,
            &[
                target,
                "--username",
                "root",
                "--password",
                "test-password",
                "--ssh-port",
                "22",
                "--web-port",
                "80",
                "--minimal",
                "--with-tools",
                "--keep-docs",
                "--keep-package-cache",
                "--dns",
                "1.1.1.1,2606:4700:4700::1111",
                "--dns-search",
                "example.internal",
            ],
            Some("https://assets.example/reinstall"),
        );
        press(&mut wizard, KeyCode::Esc);
        press(&mut wizard, KeyCode::F(2));
        let summary = wizard.summary().join("\n");
        assert!(summary.contains("Install profile: minimal"));
        assert!(summary.contains("1.1.1.1"));
        assert!(summary.contains("example.internal"));
    }
}

#[test]
fn inactive_dns_and_minimal_overrides_are_not_submitted_or_exposed_for_rescue() {
    let mut wizard = wizard();
    choose(&mut wizard, 0);
    credentials(&mut wizard);
    set(&mut wizard, Field::DnsServers, "not an IP");
    set(&mut wizard, Field::DnsSearch, "invalid/domain");
    wizard.draft_mut().keep_docs = true;
    wizard.draft_mut().keep_package_cache = true;
    let plan = wizard.build_plan().unwrap();
    assert!(plan.system.is_default());
    wizard.draft_mut().dns_mode = DnsMode::Static;
    assert!(wizard.build_plan().is_err());
    for index in [4, 5, 6, 7] {
        choose(&mut wizard, index);
        assert!(!wizard.entries().iter().any(|entry| matches!(
            entry.action,
            Action::Open(Page::Installation | Page::Network)
        )));
    }
}
