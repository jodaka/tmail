//! Account configuration wizard screens (ADR 0003 §3.2 W1–W7).
//!
//! A full-screen panel: during first run the shell has no data to show,
//! so the wizard replaces the whole chrome (topbar, sidebar, status bar)
//! and the renderer dispatches straight here while `AppState.session.wizard` is
//! set. Layout: a 3-row title band (text vertically centered), the step
//! content centered vertically and horizontally in between, and a 3-row
//! status band with the hotkeys, separated from the body by a horizontal
//! rule. Text inputs render as bordered boxes with the label as the
//! block title and a surface background (the ratatui input pattern);
//! the focused box gets the accent border. All styling goes through the
//! shared theme tokens, so theme switching keeps working; the raw
//! password renders masked and the focused field draws the same inline
//! caret as the composer.

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Borders, Padding, Paragraph};
use unicode_width::UnicodeWidthStr;

use crate::app::state::AppState;
use crate::app::wizard::{NameChoice, StorageMode, WizardState, WizardStep};
use crate::ui::components::spinner;
use crate::ui::theme::Theme;

/// Width of the centered content column (fields, recaps, lists): wide
/// enough for a full discovery row (label + both URLs) without wrapping.
const CONTENT_WIDTH: u16 = 80;

/// Height of one bordered input box (top border + text + bottom border).
const INPUT_HEIGHT: u16 = 3;

/// Chrome rows: title band (3) + separator (1) + status band (3).
const CHROME_ROWS: u16 = 7;

/// Render the wizard over the whole terminal area. (Keyboard-first: the
/// wizard records no click targets in v1 — mouse input is inert here.)
pub fn render(frame: &mut Frame<'_>, area: Rect, state: &AppState, theme: &Theme) {
    let Some(wizard) = &state.session.wizard else {
        return;
    };
    if area.width < 46 || area.height < CHROME_ROWS + 4 {
        render_too_small(frame, area, theme);
        return;
    }

    // Bands: title (3) / body (rest) / separator (1) / status (3).
    let body_height = area.height - CHROME_ROWS;
    let body = Rect {
        x: area.x,
        y: area.y + 3,
        width: area.width,
        height: body_height,
    };
    let separator = Rect {
        x: area.x,
        y: body.y + body_height,
        width: area.width,
        height: 1,
    };
    let status = Rect {
        x: area.x,
        y: separator.y + 1,
        width: area.width,
        height: 3,
    };

    // Title band: the text sits on the middle row (vertically centered),
    // horizontally centered to match the content column.
    frame.render_widget(
        Paragraph::new(Line::from(Span::styled(
            step_title(wizard.step),
            Style::new().fg(theme.text).add_modifier(Modifier::BOLD),
        )))
        .centered(),
        Rect {
            x: area.x,
            y: area.y + 1,
            width: area.width,
            height: 1,
        },
    );

    // Step content, centered vertically and horizontally.
    render_step(frame, body, state, wizard, theme);

    // The rule separating the body from the status bar.
    frame.render_widget(
        Block::new()
            .borders(Borders::TOP)
            .border_style(theme.hairline()),
        separator,
    );

    // Status band: the hotkey row sits on the middle row (vertically
    // centered), horizontally centered.
    frame.render_widget(
        Paragraph::new(Line::from(hint_spans(wizard, theme))).centered(),
        Rect {
            x: area.x,
            y: status.y + 1,
            width: area.width,
            height: 1,
        },
    );
}

fn step_title(step: WizardStep) -> &'static str {
    match step {
        WizardStep::Email => "Account setup — email address",
        WizardStep::Discovery => "Account setup — server settings",
        WizardStep::Identity => "Account setup — identity",
        WizardStep::Credentials => "Account setup — sign-in",
        WizardStep::Testing => "Account setup — testing",
        WizardStep::Confirm => "Account setup — confirm",
        WizardStep::Saved => "Account setup — done",
    }
}

/// The status-bar hotkey row (ADR 0003 feedback: "Esc steps back" lives
/// here with the other chords; the initial screen advertises Ctrl+C as
/// quit instead of the foolish "esc quit" pair).
fn hint_spans<'a>(wizard: &WizardState, theme: &'a Theme) -> Vec<Span<'a>> {
    let hints: &[&str] = match wizard.step {
        WizardStep::Email => &["↵ detect settings", "Esc cancel", "Ctrl+C quit"],
        WizardStep::Discovery if wizard.discovery.override_open => {
            &["Tab next field", "↵ use servers", "Esc steps back"]
        }
        WizardStep::Discovery => &[
            "↑↓ choose",
            "↵ accept",
            "e manual",
            "r retry",
            "Esc steps back",
        ],
        WizardStep::Identity => &["↵ continue", "Esc steps back"],
        WizardStep::Credentials if wizard.credentials.credentials_index == 1 => {
            &["Tab next field", "↵/space switch storage", "Esc steps back"]
        }
        WizardStep::Credentials => &["Tab next field", "↵ test connection", "Esc steps back"],
        WizardStep::Testing => &["Esc cancel test"],
        WizardStep::Confirm => &["↑↓ choose", "↵ save account", "Esc steps back"],
        WizardStep::Saved => &["↵ continue"],
    };
    let mut spans = Vec::new();
    for (index, hint) in hints.iter().enumerate() {
        if index > 0 {
            spans.push(Span::styled("  ·  ", Style::new().fg(theme.dim)));
        }
        spans.push(Span::styled(*hint, Style::new().fg(theme.text_soft)));
    }
    spans
}

/// The centered content column inside the body band.
fn content_column(body: Rect) -> Rect {
    let width = CONTENT_WIDTH.min(body.width.saturating_sub(2));
    Rect {
        x: body.x + (body.width.saturating_sub(width)) / 2,
        y: body.y,
        width,
        height: body.height,
    }
}

/// Vertically centers `content_height` rows inside `column`, returning
/// the start row.
fn centered_y(column: Rect, content_height: u16) -> u16 {
    column.y + column.height.saturating_sub(content_height) / 2
}

// ── Step content ─────────────────────────────────────────────────────────

fn render_step(
    frame: &mut Frame<'_>,
    body: Rect,
    state: &AppState,
    wizard: &WizardState,
    theme: &Theme,
) {
    match wizard.step {
        WizardStep::Email => render_email(frame, body, wizard, theme),
        WizardStep::Discovery => render_discovery(
            frame,
            body,
            wizard,
            theme,
            crate::ui::components::spinner::pane_millis(state),
        ),
        WizardStep::Identity => render_identity(frame, body, wizard, theme),
        WizardStep::Credentials => render_credentials(frame, body, wizard, theme),
        WizardStep::Testing => render_testing(frame, body, state, theme),
        WizardStep::Confirm => render_confirm(frame, body, wizard, theme),
        WizardStep::Saved => render_saved(frame, body, wizard, theme),
    }
}

fn render_email(frame: &mut Frame<'_>, body: Rect, wizard: &WizardState, theme: &Theme) {
    let column = content_column(body);
    let error_height = u16::from(wizard.last_error.is_some());
    let content_height = 2 + 1 + INPUT_HEIGHT + 1 + error_height;
    let mut y = centered_y(column, content_height);

    intro(
        frame,
        column,
        &mut y,
        theme,
        "tmail detects the IMAP/SMTP settings for your address, tests the",
    );
    intro(
        frame,
        column,
        &mut y,
        theme,
        "sign-in, and saves the account into the shared himalaya config.",
    );
    y += 1;
    // The email field is the screen's only control: it is focused
    // whenever the email step is active, so it carries the accent
    // border and the inline caret (the composer's cursor convention).
    input_box(
        frame,
        column,
        y,
        "Email",
        value_spans(
            &wizard.email.address.value,
            wizard.email.address.cursor,
            true,
            false,
            input_inner_width(column),
            theme,
        ),
        true,
        theme,
    );
    y += INPUT_HEIGHT + 1;
    error_line(frame, column, y, wizard.last_error.as_deref(), theme);
}

fn render_discovery(
    frame: &mut Frame<'_>,
    body: Rect,
    wizard: &WizardState,
    theme: &Theme,
    now_millis: u64,
) {
    let column = content_column(body);

    if wizard.discovery.discovering {
        // Scanner loader + message; the loader animates from the wall
        // clock (deterministic in tests).
        let email = wizard.email.address.value.trim();
        let y = centered_y(column, 1);
        spinner::render_blocks(
            frame,
            Rect {
                x: column.x,
                y,
                width: 8,
                height: 1,
            },
            theme.accent,
            theme.background,
            now_millis,
        );
        frame.render_widget(
            Paragraph::new(Span::styled(
                format!("  Detecting settings for {email}…"),
                Style::new().fg(theme.text),
            )),
            Rect {
                x: column.x + 8,
                y,
                width: column.width.saturating_sub(8),
                height: 1,
            },
        );
        return;
    }

    if wizard.discovery.override_open {
        let error_height = u16::from(wizard.last_error.is_some());
        let content_height =
            2 + INPUT_HEIGHT + 1 + INPUT_HEIGHT + 1 + INPUT_HEIGHT + 1 + error_height;
        let mut y = centered_y(column, content_height);
        intro(
            frame,
            column,
            &mut y,
            theme,
            "Discovery found nothing usable — enter the servers exactly as your",
        );
        intro(
            frame,
            column,
            &mut y,
            theme,
            "provider publishes them (scheme://host:port).",
        );
        y += 1;
        let labels = ["IMAP server", "SMTP server", "Username"];
        for (index, label) in labels.iter().enumerate() {
            let field = &wizard.discovery.override_fields[index];
            input_box(
                frame,
                column,
                y,
                label,
                value_spans(
                    &field.value,
                    field.cursor,
                    wizard.discovery.override_index == index,
                    false,
                    input_inner_width(column),
                    theme,
                ),
                wizard.discovery.override_index == index,
                theme,
            );
            y += INPUT_HEIGHT + 1;
        }
        error_line(frame, column, y, wizard.last_error.as_deref(), theme);
        return;
    }

    if wizard.discovery.services.is_empty() {
        let error_height = u16::from(wizard.last_error.is_some());
        let content_height = 1 + 1 + error_height;
        let mut y = centered_y(column, content_height);
        intro(
            frame,
            column,
            &mut y,
            theme,
            "No settings were found. Press e to enter the servers manually.",
        );
        y += 1;
        error_line(frame, column, y, wizard.last_error.as_deref(), theme);
        return;
    }

    let error_height = u16::from(wizard.last_error.is_some());
    let content_height = 1 + 1 + wizard.discovery.services.len() as u16 + 1 + error_height;
    let mut y = centered_y(column, content_height);
    intro(
        frame,
        column,
        &mut y,
        theme,
        "Detected settings — choose a candidate:",
    );
    y += 1;
    for (index, service) in wizard.discovery.services.iter().enumerate() {
        if y >= column.y + column.height {
            break;
        }
        let selected = index == wizard.discovery.service_index;
        let marker = if selected { "›" } else { " " };
        let smtp = match &service.smtp {
            Some(smtp) => smtp.url.as_str(),
            None => "smtp: not found",
        };
        let label_style = if selected {
            theme.accent_fg()
        } else {
            Style::new().fg(theme.text_soft)
        };
        let detail_style = if selected {
            theme.accent_fg()
        } else {
            Style::new().fg(theme.dim)
        };
        frame.render_widget(
            Paragraph::new(Line::from(vec![
                Span::styled(
                    format!("{marker} "),
                    if selected {
                        theme.accent_fg()
                    } else {
                        Style::new().fg(theme.dim)
                    },
                ),
                Span::styled(service.source.label(), label_style),
                Span::styled(format!(" {} {smtp}", service.imap.url), detail_style),
            ]))
            .style(if selected {
                theme.row_selected()
            } else {
                Style::new()
            }),
            Rect {
                x: column.x,
                y,
                width: column.width,
                height: 1,
            },
        );
        y += 1;
    }
    y += 1;
    error_line(frame, column, y, wizard.last_error.as_deref(), theme);
}

fn render_identity(frame: &mut Frame<'_>, body: Rect, wizard: &WizardState, theme: &Theme) {
    let column = content_column(body);
    let service = wizard.selected_service();
    let imap = service.map(|s| s.imap.url.as_str()).unwrap_or("-");
    let smtp = service
        .and_then(|s| s.smtp.as_ref())
        .map(|smtp| smtp.url.as_str())
        .unwrap_or("smtp: not found");
    let content_height = INPUT_HEIGHT + 1 + 3;
    let mut y = centered_y(column, content_height);

    input_box(
        frame,
        column,
        y,
        "Name",
        value_spans(
            &wizard.credentials.display_name.value,
            wizard.credentials.display_name.cursor,
            true,
            false,
            input_inner_width(column),
            theme,
        ),
        true,
        theme,
    );
    y += INPUT_HEIGHT + 1;
    recap(
        frame,
        column,
        &mut y,
        "Email",
        wizard.email.address.value.trim(),
        theme,
    );
    recap(frame, column, &mut y, "IMAP", imap, theme);
    recap(frame, column, &mut y, "SMTP", smtp, theme);
}

fn render_credentials(frame: &mut Frame<'_>, body: Rect, wizard: &WizardState, theme: &Theme) {
    let column = content_column(body);
    let gmail =
        wizard.discovery.gmail_hint || wizard.provider() == Some(crate::discovery::Provider::Gmail);
    let gmail_height = if gmail { 2 } else { 0 };
    let error_height = u16::from(wizard.last_error.is_some());
    let content_height =
        INPUT_HEIGHT + 1 + INPUT_HEIGHT + 1 + INPUT_HEIGHT + 1 + gmail_height + error_height;
    let mut y = centered_y(column, content_height);

    input_box(
        frame,
        column,
        y,
        "Username",
        value_spans(
            &wizard.credentials.username.value,
            wizard.credentials.username.cursor,
            wizard.credentials.credentials_index == 0,
            false,
            input_inner_width(column),
            theme,
        ),
        wizard.credentials.credentials_index == 0,
        theme,
    );
    y += INPUT_HEIGHT + 1;

    // The storage-mode toggle as its own box (checkbox row inside).
    let storage_focused = wizard.credentials.credentials_index == 1;
    let storage = Rect {
        x: column.x,
        y,
        width: column.width,
        height: INPUT_HEIGHT,
    };
    let block = input_block("Password storage", storage_focused, theme);
    let inner = block.inner(storage);
    frame.render_widget(block, storage);
    let raw = toggle_chip(
        "store password in config",
        wizard.credentials.storage_mode == StorageMode::Raw,
        storage_focused,
        theme,
    );
    let cmd = toggle_chip(
        "fetch via command",
        wizard.credentials.storage_mode == StorageMode::Command,
        storage_focused,
        theme,
    );
    frame.render_widget(
        Paragraph::new(Line::from(vec![raw, Span::raw("  "), cmd])),
        inner,
    );
    y += INPUT_HEIGHT + 1;

    // The secret field: masked in raw mode, the command in command mode.
    match wizard.credentials.storage_mode {
        StorageMode::Raw => {
            let masked: String =
                "•".repeat(wizard.credentials.password.value.chars().count().min(64));
            input_box(
                frame,
                column,
                y,
                "Password",
                value_spans(
                    &masked,
                    wizard.credentials.password.cursor,
                    wizard.credentials.credentials_index == 2,
                    true,
                    input_inner_width(column),
                    theme,
                ),
                wizard.credentials.credentials_index == 2,
                theme,
            );
        }
        StorageMode::Command => {
            input_box(
                frame,
                column,
                y,
                "Command",
                value_spans(
                    &wizard.credentials.command.value,
                    wizard.credentials.command.cursor,
                    wizard.credentials.credentials_index == 2,
                    false,
                    input_inner_width(column),
                    theme,
                ),
                wizard.credentials.credentials_index == 2,
                theme,
            );
        }
    }
    y += INPUT_HEIGHT + 1;

    // Gmail app-password hint (ADR 0003 §3.2 W4).
    if gmail {
        intro(
            frame,
            column,
            &mut y,
            theme,
            "Gmail: IMAP needs an app password (a Google account with 2FA) — not",
        );
        intro(
            frame,
            column,
            &mut y,
            theme,
            "your normal sign-in password.",
        );
    }
    error_line(frame, column, y, wizard.last_error.as_deref(), theme);
}

fn render_testing(frame: &mut Frame<'_>, body: Rect, state: &AppState, theme: &Theme) {
    let column = content_column(body);
    let y = centered_y(column, 1);
    spinner::render_blocks(
        frame,
        Rect {
            x: column.x,
            y,
            width: 8,
            height: 1,
        },
        theme.accent,
        theme.background,
        crate::ui::components::spinner::pane_millis(state),
    );
    frame.render_widget(
        Paragraph::new(Span::styled(
            "  Testing IMAP connection…",
            Style::new().fg(theme.text),
        )),
        Rect {
            x: column.x + 8,
            y,
            width: column.width.saturating_sub(8),
            height: 1,
        },
    );
}

fn render_confirm(frame: &mut Frame<'_>, body: Rect, wizard: &WizardState, theme: &Theme) {
    let column = content_column(body);
    let service = wizard.selected_service();
    let imap = service.map(|s| s.imap.url.as_str()).unwrap_or("-");
    let smtp = service
        .and_then(|s| s.smtp.as_ref())
        .map(|smtp| smtp.url.as_str())
        .unwrap_or("smtp: not found");
    let secret = match wizard.credentials.storage_mode {
        StorageMode::Raw => String::from("stored in config (****)"),
        StorageMode::Command => format!("via command: {}", wizard.credentials.command.value),
    };
    let choice_height = wizard.confirm.name_choice.as_ref().map(|_| 3).unwrap_or(0);
    let warning_height = u16::from(
        wizard.config.existing_shared_readable
            && wizard.credentials.storage_mode == StorageMode::Raw,
    );
    let error_height = u16::from(wizard.last_error.is_some());
    let content_height = 8
        + 1
        + (wizard.confirm.aliases.len() as u16 + 1)
        + 1
        + choice_height
        + warning_height
        + error_height;
    let mut y = centered_y(column, content_height);

    for (label, value) in [
        (
            "Account",
            format!("[accounts.{}]", wizard.confirm.account_name),
        ),
        ("Email", wizard.email.address.value.trim().to_string()),
        (
            "Name",
            wizard.credentials.display_name.value.trim().to_string(),
        ),
        ("IMAP", imap.to_string()),
        ("SMTP", smtp.to_string()),
        ("Password", secret),
        (
            "Default",
            if wizard.will_set_default() {
                String::from("yes")
            } else {
                String::from("no (existing default stays)")
            },
        ),
        (
            "Config",
            wizard
                .config
                .save_path
                .as_ref()
                .map(|path| path.display().to_string())
                .unwrap_or_else(|| String::from("-")),
        ),
    ] {
        recap(frame, column, &mut y, label, &value, theme);
    }

    // Derived aliases (ADR 0003 §3.5).
    if !wizard.confirm.aliases.is_empty() {
        y += 1;
        for (role, mailbox) in &wizard.confirm.aliases {
            if y >= column.y + column.height {
                break;
            }
            recap(frame, column, &mut y, role, mailbox, theme);
        }
    }

    // Name-collision choice (ADR 0003 §3.6): replace by default, or save
    // under the -2 suffix.
    if let Some(choice) = &wizard.confirm.name_choice {
        y += 1;
        let replace = choice_row("replace the existing account", 0, wizard, theme);
        let suffix = match choice {
            NameChoice::Suffix(suffix) => format!("save as [accounts.{suffix}]"),
            NameChoice::Replace => String::from("save as …"),
        };
        let suffix = choice_row(&suffix, 1, wizard, theme);
        frame.render_widget(
            Paragraph::new(Line::from(vec![replace, Span::raw("  "), suffix])),
            Rect {
                x: column.x,
                y,
                width: column.width,
                height: 1,
            },
        );
        y += 2;
    }

    // Shared-readable warning (ADR 0003 §3.6): raw secrets only.
    if wizard.config.existing_shared_readable && wizard.credentials.storage_mode == StorageMode::Raw
    {
        intro(
            frame,
            column,
            &mut y,
            theme,
            "warning: existing config is readable by others; run chmod 600",
        );
    }
    error_line(frame, column, y, wizard.last_error.as_deref(), theme);
}

fn render_saved(frame: &mut Frame<'_>, body: Rect, wizard: &WizardState, theme: &Theme) {
    let column = content_column(body);
    let path = wizard
        .confirm
        .saved_path
        .as_ref()
        .map(|path| path.display().to_string())
        .unwrap_or_else(|| String::from("-"));
    let warning_height = u16::from(wizard.confirm.permissions_warning.is_some());
    let created_height = u16::from(wizard.confirm.saved_created);
    let content_height = 1 + created_height + warning_height + 1 + 1;
    let mut y = centered_y(column, content_height);

    intro(
        frame,
        column,
        &mut y,
        theme,
        &format!("Account saved to {path}"),
    );
    if wizard.confirm.saved_created {
        intro(
            frame,
            column,
            &mut y,
            theme,
            "The config file was created with permissions 0600.",
        );
    }
    if let Some(warning) = &wizard.confirm.permissions_warning {
        intro(frame, column, &mut y, theme, warning);
    }
    y += 1;
    let next = if wizard.manual {
        "Press ↵ to exit."
    } else {
        "Press ↵ to open your mailbox."
    };
    intro(frame, column, &mut y, theme, next);
}

// ── Shared row helpers ───────────────────────────────────────────────────

/// A bordered input box (the ratatui input pattern): the label rides the
/// top border as the block title, the value area carries the surface
/// background, and the focused box gets the accent border.
fn input_block<'a>(label: &'a str, focused: bool, theme: &'a Theme) -> Block<'a> {
    let border = if focused {
        Style::new().fg(theme.accent)
    } else {
        theme.hairline()
    };
    Block::bordered()
        .border_type(BorderType::Rounded)
        .title(format!(" {label} "))
        .title_style(if focused {
            Style::new().fg(theme.accent).add_modifier(Modifier::BOLD)
        } else {
            Style::new().fg(theme.dim)
        })
        .border_style(border)
        .style(Style::new().bg(theme.surface))
        .padding(Padding::horizontal(1))
}

fn input_box(
    frame: &mut Frame<'_>,
    column: Rect,
    y: u16,
    label: &str,
    value: Vec<Span<'_>>,
    focused: bool,
    theme: &Theme,
) {
    if y + INPUT_HEIGHT > column.y + column.height {
        return;
    }
    let area = Rect {
        x: column.x,
        y,
        width: column.width,
        height: INPUT_HEIGHT,
    };
    let block = input_block(label, focused, theme);
    let inner = block.inner(area);
    frame.render_widget(block, area);
    frame.render_widget(Paragraph::new(Line::from(value)), inner);
}

/// Value spans of one field with an inline caret (the composer's
/// convention: the focused field draws the reversed-cell caret; masked
/// values arrive already masked).
fn value_spans<'a>(
    value: &'a str,
    cursor: usize,
    focused: bool,
    masked: bool,
    max_width: usize,
    theme: &'a Theme,
) -> Vec<Span<'a>> {
    let caret_style = Style::new()
        .fg(theme.background)
        .bg(theme.accent)
        .add_modifier(Modifier::BOLD);
    let normal = Style::new().fg(theme.text);
    let mut spans = Vec::new();
    let char_count = value.chars().count();
    let mut used = 0usize;
    for (index, ch) in value.chars().enumerate() {
        let width = ch.to_string().width();
        if used + width > max_width {
            break;
        }
        let style = if focused && index == cursor {
            caret_style
        } else {
            normal
        };
        // Masked fields never render the real character (ADR 0003 §3.2
        // W4: keystrokes render as *).
        let shown = if masked {
            "•".to_string()
        } else {
            ch.to_string()
        };
        spans.push(Span::styled(shown, style));
        used += width;
    }
    // The caret past the end of the text: a reversed space (the
    // composer's cursor convention — the terminal cursor stays hidden
    // app-wide).
    if focused && cursor >= char_count && used < max_width {
        spans.push(Span::styled(" ", caret_style));
    }
    spans
}

/// The writable width inside one input box (borders + horizontal
/// padding).
fn input_inner_width(column: Rect) -> usize {
    column.width.saturating_sub(4) as usize
}

fn toggle_chip<'a>(label: &'a str, active: bool, row_selected: bool, theme: &'a Theme) -> Span<'a> {
    let marker = if active { "[x]" } else { "[ ]" };
    if row_selected && active {
        Span::styled(
            format!("{marker} {label}"),
            Style::new()
                .fg(theme.background)
                .bg(theme.accent)
                .add_modifier(Modifier::BOLD),
        )
    } else if active {
        Span::styled(format!("{marker} {label}"), Style::new().fg(theme.text))
    } else {
        Span::styled(format!("{marker} {label}"), Style::new().fg(theme.dim))
    }
}

fn choice_row<'a>(
    label: &'a str,
    index: usize,
    wizard: &WizardState,
    theme: &'a Theme,
) -> Span<'a> {
    let selected = wizard.confirm.name_choice_index == index;
    let marker = if selected { "(•)" } else { "( )" };
    if selected {
        Span::styled(
            format!("{marker} {label}"),
            Style::new()
                .fg(theme.background)
                .bg(theme.accent)
                .add_modifier(Modifier::BOLD),
        )
    } else {
        Span::styled(
            format!("{marker} {label}"),
            Style::new().fg(theme.text_soft),
        )
    }
}

/// A dim label + value line inside the content column.
fn recap(
    frame: &mut Frame<'_>,
    column: Rect,
    y: &mut u16,
    label: &str,
    value: &str,
    theme: &Theme,
) {
    if *y >= column.y + column.height {
        return;
    }
    frame.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled(format!("{label:>8}"), Style::new().fg(theme.dim)),
            Span::raw("  "),
            Span::styled(value, Style::new().fg(theme.text)),
        ])),
        Rect {
            x: column.x,
            y: *y,
            width: column.width,
            height: 1,
        },
    );
    *y += 1;
}

fn intro(frame: &mut Frame<'_>, column: Rect, y: &mut u16, theme: &Theme, text: &str) {
    if *y >= column.y + column.height {
        return;
    }
    frame.render_widget(
        Paragraph::new(Span::styled(text, Style::new().fg(theme.text_soft))),
        Rect {
            x: column.x,
            y: *y,
            width: column.width,
            height: 1,
        },
    );
    *y += 1;
}

fn error_line(frame: &mut Frame<'_>, column: Rect, y: u16, error: Option<&str>, theme: &Theme) {
    if y >= column.y + column.height {
        return;
    }
    let Some(error) = error else {
        return;
    };
    frame.render_widget(
        Paragraph::new(Span::styled(
            format!("⚠ {error}"),
            Style::new().fg(theme.warning),
        )),
        Rect {
            x: column.x,
            y,
            width: column.width,
            height: 1,
        },
    );
}

fn render_too_small(frame: &mut Frame<'_>, area: Rect, theme: &Theme) {
    let message = format!(
        "Terminal too small ({}×{})\ntmail's wizard needs at least 46×11 columns/rows.\nEnlarge the window or press Ctrl+C to quit.",
        area.width, area.height
    );
    let lines: Vec<Line<'_>> = message
        .lines()
        .map(|l| Line::from(Span::styled(l, Style::new().fg(theme.text))))
        .collect();
    frame.render_widget(Paragraph::new(lines).centered(), area);
}
