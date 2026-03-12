use std::process::Command;

/// Represents a detected graphical session user.
pub struct SessionUser {
    pub uid: u32,
    pub username: String,
}

/// Extract session IDs from `loginctl list-sessions --no-legend` output.
/// Each line starts with a session ID as the first whitespace-delimited field.
pub fn parse_session_list(output: &str) -> Vec<String> {
    output
        .lines()
        .filter_map(|line| {
            let id = line.split_whitespace().next()?;
            if id.is_empty() {
                None
            } else {
                Some(id.to_string())
            }
        })
        .collect()
}

/// Parse `loginctl show-session <id> -p Type -p Name -p User` output.
/// Expects `Key=Value` lines (without `--value` flag). Order-independent.
/// Returns (session_type, username, uid) if all three properties are found.
pub fn parse_session_properties(output: &str) -> Option<(String, String, u32)> {
    let mut session_type = None;
    let mut username = None;
    let mut uid = None;

    for line in output.lines() {
        let line = line.trim();
        if let Some(val) = line.strip_prefix("Type=") {
            session_type = Some(val.to_string());
        } else if let Some(val) = line.strip_prefix("Name=") {
            username = Some(val.to_string());
        } else if let Some(val) = line.strip_prefix("User=") {
            uid = val.parse::<u32>().ok();
        }
    }

    Some((session_type?, username?, uid?))
}

/// Detect the graphical session owner via loginctl.
/// Returns None if no wayland or x11 session is found, or if loginctl is unavailable.
pub fn detect_graphical_session_user() -> Option<SessionUser> {
    let output = Command::new("loginctl")
        .args(["list-sessions", "--no-legend"])
        .output()
        .ok()?;
    let stdout = String::from_utf8_lossy(&output.stdout);
    let session_ids = parse_session_list(&stdout);

    for session_id in &session_ids {
        let show_output = Command::new("loginctl")
            .args([
                "show-session",
                session_id,
                "-p",
                "Type",
                "-p",
                "Name",
                "-p",
                "User",
            ])
            .output()
            .ok()?;
        let props = String::from_utf8_lossy(&show_output.stdout);
        if let Some((session_type, username, uid)) = parse_session_properties(&props) {
            if session_type == "wayland" || session_type == "x11" {
                return Some(SessionUser { uid, username });
            }
        }
    }
    None
}

/// Discover NIRI_SOCKET by finding the socket file in XDG_RUNTIME_DIR.
/// Niri creates its socket at `$XDG_RUNTIME_DIR/niri.{display}.{pid}.sock`.
pub fn discover_niri_socket(uid: u32) -> Option<String> {
    let runtime_dir = format!("/run/user/{}", uid);
    let entries = std::fs::read_dir(&runtime_dir).ok()?;
    for entry in entries.flatten() {
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if name.starts_with("niri.") && name.ends_with(".sock") {
            return Some(entry.path().to_string_lossy().into_owned());
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_session_list_typical() {
        let output = "    2  1000 user seat0\n    5  1000 user \n";
        let ids = parse_session_list(output);
        assert_eq!(ids, vec!["2", "5"]);
    }

    #[test]
    fn test_parse_session_list_empty() {
        let ids = parse_session_list("");
        assert!(ids.is_empty());
    }

    #[test]
    fn test_parse_session_list_whitespace_only() {
        let ids = parse_session_list("   \n  \n");
        assert!(ids.is_empty());
    }

    #[test]
    fn test_parse_session_properties_wayland() {
        let output = "User=1000\nName=user\nType=wayland\n";
        let result = parse_session_properties(output);
        assert_eq!(
            result,
            Some(("wayland".to_string(), "user".to_string(), 1000))
        );
    }

    #[test]
    fn test_parse_session_properties_tty() {
        let output = "Type=tty\nName=user\nUser=1000\n";
        let result = parse_session_properties(output);
        assert_eq!(result, Some(("tty".to_string(), "user".to_string(), 1000)));
    }

    #[test]
    fn test_parse_session_properties_x11() {
        let output = "Name=john\nType=x11\nUser=1001\n";
        let result = parse_session_properties(output);
        assert_eq!(result, Some(("x11".to_string(), "john".to_string(), 1001)));
    }

    #[test]
    fn test_parse_session_properties_empty() {
        let result = parse_session_properties("");
        assert_eq!(result, None);
    }

    #[test]
    fn test_parse_session_properties_missing_field() {
        let result = parse_session_properties("Type=wayland\nName=user\n");
        assert_eq!(result, None);
    }

    #[test]
    fn test_parse_session_properties_with_extra_whitespace() {
        let output = "  Type=wayland  \n  Name=user  \n  User=1000  \n";
        let result = parse_session_properties(output);
        assert_eq!(
            result,
            Some(("wayland".to_string(), "user".to_string(), 1000))
        );
    }
}
