//! Mock/fake data for deterministic tests and snapshots (plan §20: UI
//! snapshots inject fixed data).
//!
//! Content mirrors `mockups/list.html` so visual checks compare directly.
//! All timestamps are fixed constants, which keeps rendering and snapshots
//! deterministic when tests inject a fixed `now`. The live app no longer
//! consumes mock data — it runs on the real backend since Phase 2.

use crate::app::route::{MailboxRoute, Route};
use crate::app::state::{AppState, Loadable};
use crate::domain::{
    Address, Mailbox, MailboxId, MailboxRole, Message, MessageHeaders, MessageId, MessageSummary,
    Page,
};
use chrono::{DateTime, Duration, FixedOffset, TimeZone};

/// Page size used by the mock fixtures (plan §17 default).
pub const PAGE_SIZE: usize = 20;

const TZ_PLUS_3: FixedOffset = match FixedOffset::east_opt(3 * 3600) {
    Some(tz) => tz,
    None => unreachable!(),
};

/// The mock "current time" the UI clock shows; message timestamps are
/// authored relative to it. `2026-09-02 10:47:00 +03:00`.
pub fn now() -> DateTime<FixedOffset> {
    DateTime::from_timestamp(1_788_335_220, 0)
        .expect("valid epoch")
        .with_timezone(&TZ_PLUS_3)
}

fn ts(minutes_ago: i64) -> DateTime<FixedOffset> {
    now() - Duration::minutes(minutes_ago)
}

fn days_ago(days: i64, hour: u32, minute: u32) -> DateTime<FixedOffset> {
    let d = (now() - Duration::days(days)).date_naive();
    let t = chrono::NaiveTime::from_hms_opt(hour, minute, 0).expect("valid time");
    TZ_PLUS_3
        .from_local_datetime(&chrono::NaiveDateTime::new(d, t))
        .single()
        .expect("valid local datetime")
}

fn addr(name: &str, email: &str) -> Address {
    Address {
        name: Some(String::from(name)),
        email: String::from(email),
    }
}

/// Sidebar mailboxes (no labels block: plan §4 override).
pub fn mock_mailboxes() -> Vec<Mailbox> {
    vec![
        Mailbox {
            id: MailboxId(String::from("inbox")),
            name: String::from("Inbox"),
            role: Some(MailboxRole::Inbox),
            unread_count: Some(24),
            total_count: Some(312),
        },
        Mailbox {
            id: MailboxId(String::from("sent")),
            name: String::from("Sent"),
            role: Some(MailboxRole::Sent),
            unread_count: None,
            total_count: Some(1_204),
        },
        Mailbox {
            id: MailboxId(String::from("drafts")),
            name: String::from("Drafts"),
            role: Some(MailboxRole::Drafts),
            unread_count: None,
            total_count: Some(3),
        },
        Mailbox {
            id: MailboxId(String::from("archive")),
            name: String::from("Archive"),
            role: Some(MailboxRole::Archive),
            unread_count: None,
            total_count: Some(8_742),
        },
        Mailbox {
            id: MailboxId(String::from("spam")),
            name: String::from("Spam"),
            role: Some(MailboxRole::Spam),
            unread_count: Some(5),
            total_count: Some(97),
        },
        Mailbox {
            id: MailboxId(String::from("trash")),
            name: String::from("Trash"),
            role: Some(MailboxRole::Trash),
            unread_count: None,
            total_count: Some(14),
        },
    ]
}

/// First-page rows mirroring the mockup list.
fn inbox_seed() -> Vec<MessageSummary> {
    let mid = |n: &str| MessageId(String::from(n));
    let msg = |id_: &str,
               from: Address,
               subject: &str,
               snippet: Option<&str>,
               minutes: i64,
               is_read: bool,
               starred: bool| MessageSummary {
        id: mid(id_),
        mailbox_id: MailboxId(String::from("inbox")),
        message_id: None,
        from: vec![from],
        to: Vec::new(),
        subject: String::from(subject),
        snippet: snippet.map(String::from),
        timestamp: ts(minutes),
        is_read,
        is_starred: starred,
        has_attachments: false,
    };
    vec![
        msg(
            "m1",
            addr("KKF Notifications", "notify@kitchenknifeforums.com"),
            "Re: WIP — 240 mm stainless-clad gyuto",
            Some("quench done at 760 °C, first edge tests tonight"),
            5,
            false,
            false,
        ),
        msg(
            "m2",
            addr("Maksim Orlov", "maksim@example.com"),
            "Re: gyuto for September",
            Some("no rush, a stabilized birch handle would be perfect"),
            92,
            false,
            false,
        ),
        msg(
            "m3",
            addr("Takefu Special Steel", "sales@takefu-special-steel.example"),
            "Shipment #TF-8841 · Shirogami #2, 3.5 mm",
            Some("customs clearance in Rotterdam, ETA Friday"),
            164,
            false,
            false,
        ),
        msg(
            "m4",
            addr("PayPal", "service@paypal.example"),
            "Payment received · $650.00 from M. Orlov",
            Some("order #214, 50% deposit, thank you"),
            1_350,
            true,
            true,
        ),
        msg(
            "m5",
            addr("Stefan Keller", "stefan@example.ch"),
            "Re: tsukamaki thread source",
            Some("ito in dark silk, 8 mm, sending you the shop link"),
            1_420,
            true,
            true,
        ),
        msg(
            "m6",
            addr("Ilya Semyonov", "ilya@example.com"),
            "Stabilized Karelian birch blocks",
            Some("two blocks left from the batch · 32 × 40 × 150 mm"),
            1_510,
            true,
            false,
        ),
        msg(
            "m7",
            addr("KKF Notifications", "notify@kitchenknifeforums.com"),
            "New posts in “Edge retention · AEB-L vs ginsan”",
            Some("+14 replies since your last visit"),
            1_600,
            true,
            false,
        ),
        msg(
            "m8",
            addr("DHL Express", "noreply@dhl.example"),
            "Delivered · 2× diamond plates, #320 / #1200",
            Some("signed by A. KUDRIS · tracking archived"),
            2_050,
            true,
            false,
        ),
        msg(
            "m9",
            addr("Anna Koroleva", "anna@example.com"),
            "COA cards — proof attached",
            Some("letterpress proof, 125 × 85 mm, awaiting your sign-off"),
            2_940,
            true,
            false,
        ),
        msg(
            "m10",
            addr("Google", "no-reply@accounts.google.example"),
            "Security alert · new sign-in on MacBook Pro",
            Some("if this was you, you can ignore this email"),
            4_320,
            true,
            false,
        ),
        msg(
            "m11",
            addr("Maria Ivanova", "maria@example.com"),
            "Question about damascus steel care",
            Some("does the etched pattern fade after years of use?"),
            5_760,
            true,
            false,
        ),
        msg(
            "m12",
            addr("GitHub", "notifications@github.example"),
            "[anton/tmail] PR #42 · mock data generator",
            Some("opened by @akudris · 2 files changed"),
            7_200,
            true,
            false,
        ),
    ]
}

/// Deterministic filler so page 2 exists (25 inbox messages total).
fn inbox_filler(existing: usize, wanted: usize) -> Vec<MessageSummary> {
    let senders = [
        ("Nordic Tooling", "orders@nordic-tooling.example"),
        ("Kato Whetstones", "shop@kato-whetstones.example"),
        ("Vladimir Petrov", "vladimir@example.com"),
        ("Zvonimirvalid Hrvat", "zvonimir@example.hr"),
        ("Billing Bot", "billing@example.internal"),
    ];
    let subjects = [
        "Order confirmation",
        "Re: handle material quote",
        "Weekly digest",
        "Stone grit chart update",
        "Invoice #1023",
        "Re: sheath dimensions",
    ];
    (existing..wanted)
        .map(|i| MessageSummary {
            id: MessageId(format!("m{}", i + 1)),
            mailbox_id: MailboxId(String::from("inbox")),
            message_id: None,
            from: vec![addr(
                senders[i % senders.len()].0,
                senders[i % senders.len()].1,
            )],
            to: Vec::new(),
            subject: format!("{} #{}", subjects[i % subjects.len()], i + 1),
            snippet: Some(String::from("generated filler row for pagination")),
            timestamp: ts(60 * (24 + (i as i64) * 7)),
            is_read: true,
            is_starred: false,
            has_attachments: false,
        })
        .collect()
}

fn small_mailbox(mailbox: &str, count: usize) -> Vec<MessageSummary> {
    (0..count)
        .map(|i| MessageSummary {
            id: MessageId(format!("{mailbox}-{i}")),
            mailbox_id: MailboxId(String::from(mailbox)),
            message_id: None,
            from: vec![addr("Tmail Probe", "probe@tmail.local")],
            to: Vec::new(),
            subject: format!("{mailbox} message {}", i + 1),
            snippet: None,
            timestamp: days_ago(i as i64 + 1, 9, 30),
            is_read: true,
            is_starred: false,
            has_attachments: false,
        })
        .collect()
}

/// All messages of a mock mailbox, newest first (matching `envelope list`
/// ordering, ADR 0001 finding 3).
pub fn mock_messages(mailbox_id: &MailboxId) -> Vec<MessageSummary> {
    match mailbox_id.0.as_str() {
        "inbox" => {
            let mut items = inbox_seed();
            items.extend(inbox_filler(items.len(), 25));
            items
        }
        "sent" => small_mailbox("sent", 4),
        // The drafts list shows the recipient in the sender column
        // (`mailbox::message_spans`), so the mock rows carry one.
        "drafts" => {
            let to = vec![addr("Bob Smith", "bob@example.org")];
            small_mailbox("drafts", 3)
                .into_iter()
                .map(|mut row| {
                    row.to = to.clone();
                    row
                })
                .collect()
        }
        "archive" => small_mailbox("archive", 6),
        "spam" => small_mailbox("spam", 5),
        "trash" => small_mailbox("trash", 2),
        _ => Vec::new(),
    }
}

/// One explicit page of mock messages (plan §16: explicit pagination).
pub fn mock_page(mailbox_id: &MailboxId, offset: usize, limit: usize) -> Page<MessageSummary> {
    let all = mock_messages(mailbox_id);
    let total = all.len();
    let items = all.into_iter().skip(offset).take(limit).collect();
    Page {
        items,
        offset,
        limit,
        total: Some(total),
    }
}

/// State fixture: mailboxes loaded, Inbox selected, first page in place —
/// as a backend-driven session looks once startup results have applied.
pub fn mock_initial_state() -> crate::app::state::AppState {
    let mut state = AppState::initial(PAGE_SIZE);
    state.mailboxes = Loadable::Loaded(mock_mailboxes());
    state.session.routes = vec![Route::Mailbox(MailboxRoute {
        mailbox_id: MailboxId(String::from("inbox")),
    })];
    state.mailbox_selection = 0;
    state.messages = mock_page(&MailboxId(String::from("inbox")), 0, PAGE_SIZE);
    state
}

/// A full message for the given summary, as `message read` would map it.
/// The body is long enough (40 lines) to overflow a full-size reader
/// viewport, so scroll clamping is exercisable.
pub fn mock_message(summary: &MessageSummary) -> Message {
    let body: String = (1..=40).map(|i| format!("body line {i:02}\n")).collect();
    Message {
        id: summary.id.clone(),
        mailbox_id: summary.mailbox_id.clone(),
        headers: MessageHeaders {
            subject: summary.subject.clone(),
            from: summary.from.clone(),
            to: summary.to.clone(),
            cc: Vec::new(),
            bcc: Vec::new(),
            date: Some(summary.timestamp),
            message_id: summary.message_id.clone(),
            in_reply_to: None,
            references: None,
        },
        plain_body: Some(body),
        html_body: None,
        attachments: Vec::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn inbox_has_two_pages() {
        assert_eq!(mock_messages(&MailboxId(String::from("inbox"))).len(), 25);
        let page1 = mock_page(&MailboxId(String::from("inbox")), 0, PAGE_SIZE);
        assert_eq!(page1.items.len(), 20);
        assert_eq!(page1.total, Some(25));
        assert!(page1.has_next());
        let page2 = mock_page(&MailboxId(String::from("inbox")), 20, PAGE_SIZE);
        assert_eq!(page2.items.len(), 5);
        assert!(!page2.has_next());
    }

    #[test]
    fn unknown_mailbox_is_empty() {
        let empty = mock_page(&MailboxId(String::from("nope")), 0, PAGE_SIZE);
        assert!(empty.items.is_empty());
        assert_eq!(empty.total, Some(0));
    }
}
