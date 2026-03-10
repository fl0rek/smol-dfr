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

/// Parse `loginctl show-session <id> -p Type -p Name -p User --value` output.
/// Returns (session_type, username, uid) if the output has at least 3 lines.
pub fn parse_session_properties(output: &str) -> Option<(String, String, u32)> {
    let lines: Vec<&str> = output.lines().collect();
    if lines.len() < 3 {
        return None;
    }
    let session_type = lines[0].trim().to_string();
    let username = lines[1].trim().to_string();
    let uid: u32 = lines[2].trim().parse().ok()?;
    Some((session_type, username, uid))
}

/// Parse null-byte-separated environ data (like /proc/<pid>/environ)
/// and extract the value of the given variable name.
pub fn parse_environ_for_var(environ_bytes: &[u8], var_name: &str) -> Option<String> {
    let prefix = format!("{}=", var_name);
    for entry in environ_bytes.split(|&b| b == 0) {
        if let Ok(s) = std::str::from_utf8(entry) {
            if let Some(val) = s.strip_prefix(&prefix) {
                return Some(val.to_string());
            }
        }
    }
    None
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
                "--value",
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

/// Discover NIRI_SOCKET by reading the niri process's environment.
/// Uses pgrep to find niri's PID, then reads /proc/<pid>/environ.
pub fn discover_niri_socket(uid: u32) -> Option<String> {
    let output = Command::new("pgrep")
        .args(["-u", &uid.to_string(), "niri"])
        .output()
        .ok()?;
    let pid_str = String::from_utf8_lossy(&output.stdout);
    let pid = pid_str.trim().lines().next()?.trim();
    if pid.is_empty() {
        return None;
    }

    let environ = std::fs::read(format!("/proc/{}/environ", pid)).ok()?;
    parse_environ_for_var(&environ, "NIRI_SOCKET")
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
        let output = "wayland\nuser\n1000\n";
        let result = parse_session_properties(output);
        assert_eq!(
            result,
            Some(("wayland".to_string(), "user".to_string(), 1000))
        );
    }

    #[test]
    fn test_parse_session_properties_tty() {
        let output = "tty\nuser\n1000\n";
        let result = parse_session_properties(output);
        assert_eq!(
            result,
            Some(("tty".to_string(), "user".to_string(), 1000))
        );
    }

    #[test]
    fn test_parse_session_properties_x11() {
        let output = "x11\njohn\n1001\n";
        let result = parse_session_properties(output);
        assert_eq!(
            result,
            Some(("x11".to_string(), "john".to_string(), 1001))
        );
    }

    #[test]
    fn test_parse_session_properties_empty() {
        let result = parse_session_properties("");
        assert_eq!(result, None);
    }

    #[test]
    fn test_parse_session_properties_too_few_lines() {
        let result = parse_session_properties("wayland\nuser\n");
        // Only 2 non-empty lines when trailing newline is present
        // "wayland\nuser\n" splits into ["wayland", "user", ""]
        // lines[2] = "" which won't parse as u32
        assert_eq!(result, None);
    }

    #[test]
    fn test_parse_environ_for_var_found() {
        let environ =
            b"HOME=/home/user\0NIRI_SOCKET=/run/user/1000/niri/niri.sock\0TERM=xterm\0";
        let result = parse_environ_for_var(environ, "NIRI_SOCKET");
        assert_eq!(
            result,
            Some("/run/user/1000/niri/niri.sock".to_string())
        );
    }

    #[test]
    fn test_parse_environ_for_var_not_found() {
        let environ = b"HOME=/home/user\0TERM=xterm\0";
        let result = parse_environ_for_var(environ, "NIRI_SOCKET");
        assert_eq!(result, None);
    }

    #[test]
    fn test_parse_environ_for_var_empty() {
        let result = parse_environ_for_var(b"", "NIRI_SOCKET");
        assert_eq!(result, None);
    }

    #[test]
    fn test_parse_environ_for_var_partial_match() {
        // Ensure "NIRI_SOCKET_EXTRA" doesn't match "NIRI_SOCKET"
        let environ = b"NIRI_SOCKET_EXTRA=/wrong\0NIRI_SOCKET=/correct\0";
        let result = parse_environ_for_var(environ, "NIRI_SOCKET");
        assert_eq!(result, Some("/correct".to_string()));
    }

    #[test]
    fn test_parse_environ_for_var_home() {
        let environ =
            b"HOME=/home/user\0NIRI_SOCKET=/run/user/1000/niri/niri.sock\0TERM=xterm\0";
        let result = parse_environ_for_var(environ, "HOME");
        assert_eq!(result, Some("/home/user".to_string()));
    }

    // Integration-level tests for detect_graphical_session_user and discover_niri_socket
    // cannot run in CI (require root/loginctl/running niri) but the parsing functions
    // above cover the core logic.

    // Verify that detect_graphical_session_user filters for graphical sessions:
    // This is tested implicitly by parse_session_properties returning the type,
    // and detect_graphical_session_user checking for "wayland" or "x11".

    #[test]
    fn test_parse_session_properties_with_extra_whitespace() {
        let output = "  wayland  \n  user  \n  1000  \n";
        let result = parse_session_properties(output);
        assert_eq!(
            result,
            Some(("wayland".to_string(), "user".to_string(), 1000))
        );
    }
}
