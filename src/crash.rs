//! The last crash, written to `crash.log` beside the settings, so that a
//! user can attach it to an issue. Nothing is sent anywhere.

use std::panic::PanicHookInfo;
use std::path::{Path, PathBuf};

type Hook = Box<dyn Fn(&PanicHookInfo<'_>) + Sync + Send + 'static>;

/// Writes a report on every panic, before the default message.
pub fn install() {
    if let Some(dir) = crate::settings::folder() {
        let default = std::panic::take_hook();
        std::panic::set_hook(hook(dir.join("crash.log"), default));
    }
}

/// Writes the report to `file`, then hands over to `next`.
fn hook(file: PathBuf, next: Hook) -> Hook {
    Box::new(move |info| {
        let place = info.location().map(|l| format!("{}:{}:{}", l.file(), l.line(), l.column()));
        let thread = std::thread::current();
        let _ = write(&file, &report(thread.name().unwrap_or("unnamed"), &message(info.payload()), place.as_deref()));
        next(info);
    })
}

fn message(payload: &(dyn std::any::Any + Send)) -> String {
    match (payload.downcast_ref::<&str>(), payload.downcast_ref::<String>()) {
        (Some(s), _) => s.to_string(),
        (_, Some(s)) => s.clone(),
        _ => "(no message)".into(),
    }
}

fn report(thread: &str, message: &str, place: Option<&str>) -> String {
    format!(
        "Sunburst Disk Analyser {} ({} {})\n{}\nthread '{thread}' panicked at {}:\n{message}\n",
        env!("CARGO_PKG_VERSION"),
        std::env::consts::OS,
        std::env::consts::ARCH,
        chrono::Local::now().format("%Y-%m-%d %H:%M:%S %z"),
        place.unwrap_or("an unknown place"),
    )
}

fn write(file: &Path, report: &str) -> std::io::Result<()> {
    if let Some(dir) = file.parent() {
        std::fs::create_dir_all(dir)?;
    }
    std::fs::write(file, report)
}

#[cfg(test)]
mod tests {
    #[test]
    fn report_names_version_place_and_message() {
        let r = super::report("file-ops", "index out of bounds", Some("src/ops.rs:12:5"));
        assert!(r.starts_with(&format!("Sunburst Disk Analyser {} (", env!("CARGO_PKG_VERSION"))), "{r}");
        assert!(r.ends_with("thread 'file-ops' panicked at src/ops.rs:12:5:\nindex out of bounds\n"), "{r}");
    }

    #[test]
    fn a_panic_leaves_its_report() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("new/crash.log");
        let previous = std::panic::take_hook();
        std::panic::set_hook(super::hook(file.clone(), Box::new(|_| {})));
        let _ = std::panic::catch_unwind(|| panic!("boom {}", 42));
        std::panic::set_hook(previous);
        let report = std::fs::read_to_string(&file).unwrap();
        assert!(report.contains("crash.rs:"), "{report}");
        assert!(report.ends_with("\nboom 42\n"), "{report}");
    }
}
