//! Plain-text report that remote teammates paste when discovery goes wrong.

use std::fmt::Write;
use std::path::{Path, PathBuf};

use super::locate::{self, RootCheck, SteamAccount};
use super::recordings::{SessionKind, SessionSummary};

pub fn report(
    checks: &[RootCheck],
    roots: &[PathBuf],
    override_root: Option<&Path>,
    accounts: &[SteamAccount],
    sessions: &[SessionSummary],
) -> String {
    let mut s = String::new();
    // Writing to a String cannot fail.
    let _ = writeln!(s, "SteamReplay {} diagnostics", env!("CARGO_PKG_VERSION"));
    let _ = writeln!(s, "OS: {} {}", std::env::consts::OS, std::env::consts::ARCH);
    let _ = writeln!(s, "Override: {}", override_root.map_or("none".into(), |p| p.display().to_string()));

    let _ = writeln!(s, "\nChecked Steam roots:");
    for c in checks {
        let mark = if c.exists { "x" } else { " " };
        let canon = c.canonical.as_ref().map(|p| format!(" -> {}", p.display())).unwrap_or_default();
        let _ = writeln!(s, "  [{mark}] {}{canon}", c.path.display());
    }

    let _ = writeln!(s, "\nSteam roots used:");
    for root in roots {
        let _ = writeln!(s, "  {}", root.display());
        for id in locate::account_ids(root) {
            let cfg = locate::localconfig_path(root, &id);
            let rec = root.join("userdata").join(&id).join("gamerecordings");
            let _ = writeln!(s, "    userdata/{id}: gamerecordings {}", if rec.is_dir() { "present" } else { "missing" });
            match std::fs::read_to_string(&cfg) {
                Ok(text) => match locate::game_recording_block(&text) {
                    Some(block) => {
                        let _ = writeln!(s, "      GameRecording block:");
                        for line in block.lines() {
                            let _ = writeln!(s, "        {}", line.trim_end());
                        }
                    }
                    None => {
                        let _ = writeln!(s, "      localconfig.vdf has no GameRecording block");
                    }
                },
                Err(e) => {
                    let _ = writeln!(s, "      cannot read {}: {e}", cfg.display());
                }
            }
        }
    }

    let _ = writeln!(s, "\nAccounts with recordings:");
    for a in accounts {
        let export = a.export_dir.as_ref().map_or("none".into(), |p| p.display().to_string());
        let _ = writeln!(s, "  {}: {} (export: {export})", a.account_id, a.recordings_root.display());
    }

    let bg = sessions.iter().filter(|x| x.kind == SessionKind::Background).count();
    let empty = sessions.iter().filter(|x| x.duration_s == 0.0).count();
    let _ = writeln!(
        s,
        "\nSessions: {} (background {bg}, clips {}, empty {empty})",
        sessions.len(),
        sessions.len() - bg
    );
    for x in sessions {
        let _ = writeln!(
            s,
            "  {} | {} | {} | {:.0} s | {} markers",
            x.id, x.game_name, x.started_at, x.duration_s, x.marker_count
        );
    }
    s
}
