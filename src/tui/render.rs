use super::PREVIEW;
use super::state::{Field, Page, Setting, View, Wizard};
use ratatui::Frame;
use ratatui::layout::{Alignment, Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::Line;
use ratatui::widgets::{Block, List, ListItem, ListState, Paragraph, Wrap};

impl Wizard {
    pub fn render(&mut self, frame: &mut Frame) {
        let area = frame.area();
        self.usable_terminal = area.width >= 48 && area.height >= 16;
        if !self.usable_terminal {
            frame.render_widget(
                Paragraph::new(
                    "Resize the terminal to at least 48x16. Ctrl-C quits; Esc goes back.",
                )
                .wrap(Wrap { trim: false }),
                area,
            );
            return;
        }
        if self.page() == Page::Home && matches!(self.view, View::Menu) {
            self.render_home(frame);
            return;
        }
        let [banner, breadcrumb, body, status, footer] = Layout::vertical([
            Constraint::Length(2),
            Constraint::Length(2),
            Constraint::Min(4),
            Constraint::Length(3),
            Constraint::Length(2),
        ])
        .areas(area);
        frame.render_widget(
            Paragraph::new(if self.is_current(){"Configure this system without reinstalling. Settings are persisted; network and SSH services are not restarted."}else{PREVIEW})
                .wrap(Wrap { trim: false })
                .style(Style::default().fg(Color::Yellow)),
            banner,
        );
        frame.render_widget(
            Paragraph::new(self.breadcrumb())
                .wrap(Wrap { trim: false })
                .style(Style::default().fg(Color::Cyan)),
            breadcrumb,
        );
        let entries = self.entries();
        let summary = self.summary();
        let current = self.is_current();
        let context = self.context_label();
        let selector = match &self.view {
            View::Select { setting, .. } => self.choices(*setting),
            _ => Vec::new(),
        };
        let help = match &mut self.view {
            View::Menu => {
                let menu = self.frames.last_mut().expect("menu");
                render_list(
                    frame,
                    body,
                    &if menu.page == Page::Targets {
                        menu.page.title().to_owned()
                    } else {
                        format!("{} - {}", context, menu.page.title())
                    },
                    entries.iter().map(|entry| entry.label.clone()).collect(),
                    menu.selected,
                    &mut menu.offset,
                );
                "Up/Down/Tab: choose  Enter: open  Esc/Left: back  F2: review"
            }
            View::Select {
                setting,
                selected,
                offset,
            } => {
                let title = match setting {
                    Setting::Authentication => "Choose authentication",
                    Setting::Kernel => "Choose an official kernel",
                    Setting::Boot => "Choose boot mode",
                    Setting::Hold => "Choose installation hold behavior",
                    Setting::IsoMode => "Choose ISO boot method",
                    Setting::Profile => "Choose installation profile",
                    Setting::DnsMode => "Choose DNS policy",
                    Setting::Congestion => "Congestion control (target kernel support required)",
                    Setting::Qdisc => "Queue discipline (use fq with BBR)",
                    Setting::Ipv6 => "IPv6 policy (disabling requires usable IPv4)",
                    Setting::Preference => "glibc address preference (does not disable IPv6)",
                    Setting::KeyType => "SSH key type",
                    Setting::KeyCipher => "Private key cipher (requires ssh-keygen support)",
                };
                render_list(
                    frame,
                    body,
                    title,
                    selector.iter().map(|value| (*value).to_owned()).collect(),
                    *selected,
                    offset,
                );
                "Up/Down: choose  Enter: confirm  Esc: cancel"
            }
            View::Edit { slot, input } => {
                let [label, input_area, hint] = Layout::vertical([
                    Constraint::Length(2),
                    Constraint::Length(3),
                    Constraint::Min(1),
                ])
                .areas(body);
                let key_slot = matches!(
                    slot,
                    super::state::EditSlot::Key(_) | super::state::EditSlot::NewKey
                );
                frame.render_widget(
                    Paragraph::new(if current && key_slot {
                        "SSH public key / absolute local file"
                    } else {
                        slot.label()
                    })
                    .style(Style::default().add_modifier(Modifier::BOLD)),
                    label,
                );
                let password = matches!(
                    slot,
                    super::state::EditSlot::Field(Field::Password | Field::Confirm)
                );
                let block = Block::bordered().title(if password {
                    "Password editor"
                } else {
                    "Edit field"
                });
                let inner = block.inner(input_area);
                let (visible, cursor) = input.visible(usize::from(inner.width));
                frame.render_widget(Paragraph::new(visible).block(block), input_area);
                if inner.width > 0 && inner.height > 0 {
                    frame.set_cursor_position((
                        inner.x + (cursor as u16).min(inner.width - 1),
                        inner.y,
                    ));
                }
                let help = if current && key_slot {
                    "For the current system: inline SSH public key or absolute local .pub file only. Remote URLs and private keys are not accepted."
                } else {
                    slot.hint()
                };
                frame.render_widget(Paragraph::new(help).wrap(Wrap { trim: false }), hint);
                "Enter: save  Esc: cancel  Left/Right/Home/End: move  Ctrl-U: clear"
            }
            View::Review { scroll } => {
                render_text(
                    frame,
                    body,
                    if current {
                        "Review current-system changes"
                    } else {
                        "Review installation"
                    },
                    summary.join("\n"),
                    scroll,
                );
                if current {
                    "Enter: apply configuration  Esc: edit  Up/Down/PgUp/PgDn: scroll"
                } else {
                    "Enter: continue to confirmation  Esc: edit  Up/Down/PgUp/PgDn: scroll"
                }
            }
            View::Outcome {
                success,
                message,
                scroll,
            } => {
                let title = if *success {
                    "Action result: completed"
                } else {
                    "Action result: not completed"
                };
                render_text(
                    frame,
                    body,
                    title,
                    format!(
                        "{message}\n\nNo automatic retry. Return to review to edit settings or explicitly run the action again."
                    ),
                    scroll,
                );
                "Enter/Esc: return to review  Up/Down/PgUp/PgDn: scroll  Ctrl-C: quit"
            }
            View::ToolResult { message, scroll } => {
                render_text(
                    frame,
                    body,
                    "System ssh-keygen result",
                    message.clone(),
                    scroll,
                );
                "Enter/Esc: return to key settings  Up/Down/PgUp/PgDn: scroll  Ctrl-C: quit"
            }
            View::Leaving { selected, offset } => {
                let [message, choices, hint] = Layout::vertical([
                    Constraint::Length(3),
                    Constraint::Min(5),
                    Constraint::Length(3),
                ])
                .areas(body);
                frame.render_widget(
                    Paragraph::new("Save this directory's configuration before going back?")
                        .wrap(Wrap { trim: false }),
                    message,
                );
                render_list(
                    frame,
                    choices,
                    "Return to previous menu",
                    vec![
                        "Save draft and go back".into(),
                        "Discard changes and go back".into(),
                        "Cancel - keep editing".into(),
                    ],
                    *selected,
                    offset,
                );
                frame.render_widget(Paragraph::new("Drafts are session-only. Discard does not undo applied settings or remove generated key files.").wrap(Wrap{trim:false}),hint);
                "Up/Down: choose  Enter: confirm  Esc: keep editing"
            }
        };
        frame.render_widget(
            Paragraph::new(self.error.as_str())
                .wrap(Wrap { trim: false })
                .style(Style::default().fg(Color::Red)),
            status,
        );
        frame.render_widget(Paragraph::new(help).wrap(Wrap { trim: false }), footer);
    }

    fn render_home(&mut self, frame: &mut Frame) {
        const LOGO: &str = r" ____  _   _ ____ _____  _    _     _     _____ ____
|  _ \| | | / ___|_   _|/ \  | |   | |   | ____|  _ \
| |_) | | | \___ \ | | / _ \ | |   | |   |  _| | |_) |
|  _ <| |_| |___) || |/ ___ \| |___| |___| |___|  _ <
|_| \_\\___/|____/ |_/_/   \_\_____|_____|_____|_| \_\";
        const COMPACT: &str =
            "+----------------------+\n|      RUSTALLER       |\n+----------------------+";
        let area = frame.area();
        let logo = if area.width >= 60 { LOGO } else { COMPACT };
        let [_, title, subtitle, menu, status, footer, _] = Layout::vertical([
            Constraint::Min(0),
            Constraint::Length(logo.lines().count() as u16),
            Constraint::Length(2),
            Constraint::Length(5),
            Constraint::Length(2),
            Constraint::Length(2),
            Constraint::Min(0),
        ])
        .areas(area);
        let logo_width = logo
            .lines()
            .map(|line| Line::raw(line).width())
            .max()
            .unwrap_or(0) as u16;
        let logo_width = logo_width.min(title.width);
        let centered_title = Rect::new(
            title.x + (title.width - logo_width) / 2,
            title.y,
            logo_width,
            title.height,
        );
        frame.render_widget(
            Paragraph::new(logo).style(
                Style::default()
                    .fg(Color::Cyan)
                    .add_modifier(Modifier::BOLD),
            ),
            centered_title,
        );
        frame.render_widget(
            Paragraph::new("RUSTALLER | Install or configure Linux")
                .alignment(Alignment::Center)
                .wrap(Wrap { trim: false }),
            subtitle,
        );
        let width = menu.width.min(64);
        let menu = Rect::new(
            menu.x + (menu.width - width) / 2,
            menu.y,
            width,
            menu.height,
        );
        let entries = self.entries();
        let frame_state = self.frames.last_mut().expect("home menu");
        let mut state = ListState::default().with_selected(Some(frame_state.selected));
        let list = List::new(
            entries
                .into_iter()
                .map(|e| ListItem::new(Line::raw(e.label).alignment(Alignment::Center))),
        )
        .block(
            Block::bordered()
                .title("Main menu")
                .title_alignment(Alignment::Center),
        )
        .highlight_symbol("> ")
        .highlight_style(
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        );
        frame.render_stateful_widget(list, menu, &mut state);
        frame.render_widget(
            Paragraph::new(self.error.as_str())
                .alignment(Alignment::Center)
                .style(Style::default().fg(Color::Red)),
            status,
        );
        frame.render_widget(Paragraph::new("Up/Down/Tab: choose   Enter: open   Ctrl-C: quit\nDrafts are stored for this session only.").alignment(Alignment::Center),footer);
    }
}

fn render_list(
    frame: &mut Frame,
    area: Rect,
    title: &str,
    entries: Vec<String>,
    selected: usize,
    offset: &mut usize,
) {
    let mut state = ListState::default().with_selected(Some(selected));
    *state.offset_mut() = *offset;
    let list = List::new(entries.into_iter().map(ListItem::new))
        .block(Block::bordered().title(title))
        .highlight_symbol("› ")
        .highlight_style(
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        );
    frame.render_stateful_widget(list, area, &mut state);
    *offset = state.offset();
}

fn render_text(frame: &mut Frame, area: Rect, title: &str, text: String, scroll: &mut usize) {
    // Pre-wrap by display width so scroll bounds count physical terminal rows,
    // including CJK text and very long paths, rather than just source lines.
    let width = usize::from(area.width.saturating_sub(2)).max(1);
    let mut rows = Vec::new();
    for line in text.lines() {
        let mut row = String::new();
        let mut used = 0;
        for character in line.chars() {
            let size = Line::raw(character.to_string()).width();
            if used + size > width && !row.is_empty() {
                rows.push(std::mem::take(&mut row));
                used = 0;
            }
            row.push(character);
            used += size;
        }
        rows.push(row);
    }
    let visible = usize::from(area.height.saturating_sub(2));
    *scroll = (*scroll).min(rows.len().saturating_sub(visible));
    let text = rows
        .into_iter()
        .skip(*scroll)
        .take(visible)
        .collect::<Vec<_>>()
        .join("\n");
    frame.render_widget(
        Paragraph::new(text).block(Block::bordered().title(title)),
        area,
    );
}
