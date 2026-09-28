//! The logging layer: the kernel's macros through `tracing` into the reference's lines, with the
//! levels of each output.

use std::io::{self, Write};
use std::sync::{Arc, Mutex};

use heartwood_kernel::ManualClock;
use heartwood_logger::{Level, LineLayer};
use tracing_subscriber::Registry;
use tracing_subscriber::layer::SubscriberExt;

/// 2026-01-01T00:00:08.123Z.
const TIME: i64 = 1_767_225_608_123;

#[derive(Clone, Default)]
struct Buffer(Arc<Mutex<Vec<u8>>>);

impl Write for Buffer {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(buf);
        Ok(buf.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

impl Buffer {
    fn text(&self) -> String {
        String::from_utf8(self.0.lock().unwrap().clone()).unwrap()
    }
}

/// Run `log` with a console (coloured, `info`) and a file (plain, `debug`) output.
fn capture(emojify: bool, log: impl FnOnce()) -> (String, String) {
    let (console, file) = (Buffer::default(), Buffer::default());
    let layer = LineLayer::new(emojify, Arc::new(ManualClock::new(TIME)))
        .output(Level::Info, true, Box::new(console.clone()))
        .output(Level::Debug, false, Box::new(file.clone()));
    tracing::subscriber::with_default(Registry::default().with(layer), log);
    (console.text(), file.text())
}

#[test]
fn each_output_shows_its_levels() {
    let (console, file) = capture(true, || {
        heartwood_kernel::notice!("Starting Round {} :dove_of_peace:", 2);
        heartwood_kernel::debug!("a detail");
        heartwood_kernel::emergency!("stopping");
    });
    assert_eq!(
        console,
        "[2026-01-01 00:00:08.123] \u{1b}[37mNOTICE\u{1b}[39m: \u{1b}[36mStarting Round 2 🕊️\u{1b}[39m\n\
         [2026-01-01 00:00:08.123] \u{1b}[37mEMERGENCY\u{1b}[39m: \u{1b}[36mstopping\u{1b}[39m\n"
    );
    assert_eq!(
        file,
        "[2026-01-01 00:00:08.123] NOTICE: Starting Round 2 🕊️\n\
         [2026-01-01 00:00:08.123] DEBUG: a detail\n\
         [2026-01-01 00:00:08.123] EMERGENCY: stopping\n"
    );
}

#[test]
fn emoji_codes_are_removed_when_emoji_are_off() {
    let (_, file) = capture(false, || {
        heartwood_kernel::warning!("refused :warning: :unknown:")
    });
    assert_eq!(
        file,
        "[2026-01-01 00:00:08.123] WARNING: refused  :unknown:\n"
    );
}

#[test]
fn other_fields_follow_the_message_and_empty_messages_are_dropped() {
    let (_, file) = capture(true, || {
        heartwood_kernel::info!(height = 5, peer = "10.0.0.1", "a block was disregarded");
        heartwood_kernel::info!("");
        tracing::warn!("from elsewhere");
        tracing::trace!("never shown");
    });
    assert_eq!(
        file,
        "[2026-01-01 00:00:08.123] INFO: a block was disregarded height=5 peer=10.0.0.1\n\
         [2026-01-01 00:00:08.123] WARNING: from elsewhere\n"
    );
}

/// A peer's error payload with forged lines, a carriage return, other control characters (C0,
/// DEL and C1), the line and paragraph separators and bidirectional controls.
const HOSTILE: &str = "Error: x\n[2026-01-01 00:00:09.000] ERROR: forged\r\n\
                       cr\rtab\tbell\u{7} esc\u{1b}[31mred\u{1b}[39m del\u{7f} nel\u{85} \
                       csi\u{9b} rlo\u{202e}evil\u{202c} lri\u{2066}x\u{2069} rlm\u{200f} \
                       ls\u{2028}ps\u{2029} end";

/// [`HOSTILE`] as the logger writes it.
const ESCAPED: &str = "Error: x\\n[2026-01-01 00:00:09.000] ERROR: forged\\r\\n\
                       cr\\rtab\\tbell\\u0007 esc\\u001b[31mred\\u001b[39m del\\u007f nel\\u0085 \
                       csi\\u009b rlo\\u202eevil\\u202c lri\\u2066x\\u2069 rlm\\u200f \
                       ls\\u2028ps\\u2029 end";

#[test]
fn outside_text_cannot_forge_lines() {
    let (console, file) = capture(true, || {
        heartwood_kernel::info!(
            payload = HOSTILE,
            "Failed to download blocks 2-3 from 10.0.0.1:4000: {HOSTILE} :bangbang:"
        );
    });
    let text =
        format!("Failed to download blocks 2-3 from 10.0.0.1:4000: {ESCAPED} ‼️ payload={ESCAPED}");
    assert_eq!(file, format!("[2026-01-01 00:00:08.123] INFO: {text}\n"));
    assert_eq!(
        console,
        format!("[2026-01-01 00:00:08.123] \u{1b}[32mINFO\u{1b}[39m: \u{1b}[36m{text}\u{1b}[39m\n")
    );
    // One call, one line: nothing a reader or a terminal takes for a line break or a control.
    for output in [&console, &file] {
        assert_eq!(output.lines().count(), 1, "{output:?}");
        assert!(
            !output
                .trim_end_matches('\n')
                .contains(['\n', '\r', '\u{85}', '\u{2028}', '\u{202e}'])
        );
    }
}
