//! Appends N audit entries as one helper process would: `audit_writer <root> <count> <label>`.
//! Live lanes run several at once to check that writers never share a file.

use std::time::{SystemTime, UNIX_EPOCH};

use dory_core::domain::{Access, Actor, AuditEntry, AuditOutcome, Timestamp};
use dory_core::ports::AuditSink;
use dory_store::JsonlAudit;

fn now() -> Timestamp {
    let ms = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_millis();
    Timestamp(i64::try_from(ms).unwrap())
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let [root, count, label] = args.as_slice() else {
        eprintln!("usage: audit_writer ROOT COUNT LABEL");
        std::process::exit(2);
    };
    let count: u32 = count.parse().expect("COUNT is a number");
    let log = JsonlAudit::writer(root, Actor::Helper, now());
    for i in 0..count {
        let entry = AuditEntry {
            at: now(),
            actor: Actor::Helper,
            client: Some(label.clone()),
            reported_name: Some("audit_writer".into()),
            access: Access::search(&format!("query {i}"), i),
            outcome: AuditOutcome::Served { items: i },
        };
        if let Err(e) = log.append(&entry) {
            eprintln!("append failed: {e}");
            std::process::exit(1);
        }
    }
}
