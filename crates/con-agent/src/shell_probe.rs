use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

const PREFIX: &str = "__CON_SHELL_PROBE__";

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct ShellProbeTmuxContext {
    pub session_name: Option<String>,
    pub window_id: Option<String>,
    pub window_name: Option<String>,
    pub pane_id: Option<String>,
    pub pane_current_command: Option<String>,
    pub pane_current_path: Option<String>,
    pub client_tty: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct ShellProbeResult {
    pub host: Option<String>,
    pub pwd: Option<String>,
    pub term: Option<String>,
    pub term_program: Option<String>,
    pub ssh_connection: Option<String>,
    pub ssh_tty: Option<String>,
    pub tmux_env: Option<String>,
    pub nvim_listen_address: Option<String>,
    pub tmux: Option<ShellProbeTmuxContext>,
    pub facts: BTreeMap<String, String>,
}

/// Builds the probe as one physical line: a multi-line `sh -c '…'` leaves
/// interactive shells in PS2 continuation (`quote>`) while it is typed.
/// Facts use `key=value` because terminals render TAB as spaces.
pub fn build_shell_probe_command(nonce: &str) -> String {
    let tmux_facts = [
        "session_name",
        "window_id",
        "window_name",
        "pane_id",
        "pane_current_command",
        "pane_current_path",
        "client_tty",
    ]
    .map(|field| {
        format!(
            r##"con_probe_emit tmux_{field} "$(tmux display-message -p "#{{{field}}}" 2>/dev/null || printf "")""##
        )
    })
    .join("; ");
    let statements = [
        format!(r#"con_probe_emit() {{ printf "{PREFIX} %s=%s\n" "$1" "$2"; }}"#),
        format!(r#"printf "%s\n" "__CON_SHELL_PROBE_BEGIN_{nonce}__""#),
        r#"con_probe_emit host "$(hostname 2>/dev/null || uname -n 2>/dev/null || printf "")""#
            .into(),
        r#"con_probe_emit pwd "$(pwd 2>/dev/null || printf "")""#.into(),
        r#"con_probe_emit term "${TERM-}""#.into(),
        r#"con_probe_emit term_program "${TERM_PROGRAM-}""#.into(),
        r#"con_probe_emit ssh_connection "${SSH_CONNECTION-}""#.into(),
        r#"con_probe_emit ssh_tty "${SSH_TTY-}""#.into(),
        r#"con_probe_emit tmux_env "${TMUX-}""#.into(),
        r#"con_probe_emit nvim_listen_address "${NVIM_LISTEN_ADDRESS-}""#.into(),
        format!(
            r#"if [ -n "${{TMUX-}}" ] && command -v tmux >/dev/null 2>&1; then con_probe_emit tmux_available "1"; {tmux_facts}; else con_probe_emit tmux_available "0"; fi"#
        ),
        format!(r#"printf "%s\n" "__CON_SHELL_PROBE_END_{nonce}__""#),
    ];
    format!("sh -c '{}'", statements.join("; "))
}

/// True once the probe's own END marker has been printed as a full line.
pub fn shell_probe_output_complete(lines: &[String], nonce: &str) -> bool {
    let end = format!("__CON_SHELL_PROBE_END_{nonce}__");
    lines.iter().any(|line| line.trim_end() == end)
}

pub fn parse_shell_probe_lines(lines: &[String], nonce: &str) -> Result<ShellProbeResult, String> {
    let begin = format!("__CON_SHELL_PROBE_BEGIN_{nonce}__");
    let end = format!("__CON_SHELL_PROBE_END_{nonce}__");

    let end_idx = lines
        .iter()
        .rposition(|line| line.trim_end() == end)
        .ok_or_else(|| "shell probe end marker not found in pane output".to_string())?;
    let begin_idx = lines[..end_idx]
        .iter()
        .rposition(|line| line.trim_end() == begin)
        .ok_or_else(|| "shell probe begin marker not found in pane output".to_string())?;

    let mut facts = BTreeMap::new();
    for line in &lines[begin_idx + 1..end_idx] {
        let Some(fact) = line
            .trim_end()
            .strip_prefix(PREFIX)
            .and_then(|rest| rest.strip_prefix(' '))
        else {
            continue;
        };
        if let Some((key, value)) = fact.split_once('=') {
            facts.insert(key.to_string(), value.to_string());
        }
    }

    if facts.is_empty() {
        return Err("shell probe markers were present but no probe facts were parsed".to_string());
    }

    let tmux = if facts
        .get("tmux_available")
        .is_some_and(|value| value == "1")
    {
        Some(ShellProbeTmuxContext {
            session_name: optional_fact(&facts, "tmux_session_name"),
            window_id: optional_fact(&facts, "tmux_window_id"),
            window_name: optional_fact(&facts, "tmux_window_name"),
            pane_id: optional_fact(&facts, "tmux_pane_id"),
            pane_current_command: optional_fact(&facts, "tmux_pane_current_command"),
            pane_current_path: optional_fact(&facts, "tmux_pane_current_path"),
            client_tty: optional_fact(&facts, "tmux_client_tty"),
        })
    } else {
        None
    };

    Ok(ShellProbeResult {
        host: optional_fact(&facts, "host"),
        pwd: optional_fact(&facts, "pwd"),
        term: optional_fact(&facts, "term"),
        term_program: optional_fact(&facts, "term_program"),
        ssh_connection: optional_fact(&facts, "ssh_connection"),
        ssh_tty: optional_fact(&facts, "ssh_tty"),
        tmux_env: optional_fact(&facts, "tmux_env"),
        nvim_listen_address: optional_fact(&facts, "nvim_listen_address"),
        tmux,
        facts,
    })
}

fn optional_fact(facts: &BTreeMap<String, String>, key: &str) -> Option<String> {
    facts.get(key).and_then(|value| {
        let trimmed = value.trim();
        if trimmed.is_empty() {
            None
        } else {
            Some(trimmed.to_string())
        }
    })
}

#[cfg(test)]
mod tests {
    use super::{build_shell_probe_command, parse_shell_probe_lines, shell_probe_output_complete};

    fn lines(text: &[&str]) -> Vec<String> {
        text.iter().map(|line| line.to_string()).collect()
    }

    /// Screen rows for the echoed command, wrapped the way a narrow pane shows it.
    fn echoed_command_rows(nonce: &str, cols: usize) -> Vec<String> {
        let echoed = format!("~/project % {}", build_shell_probe_command(nonce));
        echoed
            .chars()
            .collect::<Vec<_>>()
            .chunks(cols)
            .map(|row| row.iter().collect())
            .collect()
    }

    #[test]
    fn build_command_is_one_line_with_nonce_markers() {
        let cmd = build_shell_probe_command("abc123");
        assert!(
            !cmd.contains('\n'),
            "multi-line input triggers PS2 continuation"
        );
        assert!(cmd.contains("__CON_SHELL_PROBE_BEGIN_abc123__"));
        assert!(cmd.contains("__CON_SHELL_PROBE_END_abc123__"));
        assert!(cmd.contains("tmux display-message -p \"#{pane_current_command}\""));
    }

    #[test]
    fn parse_probe_block_extracts_tmux_context() {
        let lines = lines(&[
            "prompt$ sh -c '...'",
            "__CON_SHELL_PROBE_BEGIN_42__",
            "__CON_SHELL_PROBE__ host=haswell",
            "__CON_SHELL_PROBE__ pwd=/home/weyl/my project",
            "__CON_SHELL_PROBE__ ssh_connection=1.2.3.4 1111 5.6.7.8 22",
            "__CON_SHELL_PROBE__ ssh_tty=",
            "__CON_SHELL_PROBE__ tmux_env=/tmp/tmux-1000/default,123,0",
            "__CON_SHELL_PROBE__ tmux_available=1",
            "__CON_SHELL_PROBE__ tmux_session_name=work",
            "__CON_SHELL_PROBE__ tmux_window_id=@3",
            "__CON_SHELL_PROBE__ tmux_pane_id=%17",
            "__CON_SHELL_PROBE__ tmux_pane_current_command=nvim",
            "__CON_SHELL_PROBE_END_42__",
        ]);

        let result = parse_shell_probe_lines(&lines, "42").expect("probe parses");
        assert_eq!(result.host.as_deref(), Some("haswell"));
        assert_eq!(result.pwd.as_deref(), Some("/home/weyl/my project"));
        assert_eq!(result.ssh_tty, None);
        assert_eq!(
            result.ssh_connection.as_deref(),
            Some("1.2.3.4 1111 5.6.7.8 22")
        );
        let tmux = result.tmux.expect("tmux context present");
        assert_eq!(tmux.session_name.as_deref(), Some("work"));
        assert_eq!(tmux.window_id.as_deref(), Some("@3"));
        assert_eq!(tmux.pane_id.as_deref(), Some("%17"));
        assert_eq!(tmux.pane_current_command.as_deref(), Some("nvim"));
    }

    #[test]
    fn echoed_command_is_not_mistaken_for_probe_output() {
        for cols in [40, 80, 135] {
            let mut screen = echoed_command_rows("n1", cols);
            assert!(!shell_probe_output_complete(&screen, "n1"), "cols={cols}");
            assert!(
                parse_shell_probe_lines(&screen, "n1").is_err(),
                "cols={cols}"
            );

            screen.extend(lines(&[
                "__CON_SHELL_PROBE_BEGIN_n1__",
                "__CON_SHELL_PROBE__ host=haswell",
                "__CON_SHELL_PROBE__ tmux_available=0",
                "__CON_SHELL_PROBE_END_n1__",
                "~/project %",
            ]));
            assert!(shell_probe_output_complete(&screen, "n1"), "cols={cols}");
            let result = parse_shell_probe_lines(&screen, "n1").expect("probe parses");
            assert_eq!(result.host.as_deref(), Some("haswell"), "cols={cols}");
            assert_eq!(result.tmux, None);
        }
    }

    #[test]
    fn parse_probe_requires_markers() {
        let lines = vec!["no markers".to_string()];
        let err = parse_shell_probe_lines(&lines, "x").expect_err("missing markers");
        assert!(err.contains("marker"));
    }
}
