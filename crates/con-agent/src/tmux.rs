use serde::{Deserialize, Serialize};

const PREFIX: &str = "__CON_TMUX__";

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct TmuxPaneInfo {
    pub session_name: String,
    pub window_id: String,
    pub window_index: String,
    pub window_name: String,
    pub window_target: String,
    pub pane_id: String,
    pub pane_index: String,
    pub target: String,
    pub pane_active: bool,
    pub window_active: bool,
    pub pane_current_command: Option<String>,
    pub pane_current_path: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct TmuxSnapshot {
    pub panes: Vec<TmuxPaneInfo>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct TmuxCapture {
    pub target: String,
    pub content: String,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum TmuxExecLocation {
    NewWindow,
    SplitHorizontal,
    SplitVertical,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct TmuxExecResult {
    pub location: TmuxExecLocation,
    pub detached: bool,
    pub session_name: String,
    pub window_id: String,
    pub window_index: String,
    pub window_name: String,
    pub window_target: String,
    pub pane_id: String,
    pub pane_index: String,
    pub target: String,
}

fn shell_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\"'\"'"))
}

fn shell_wrap_script(script: impl AsRef<str>) -> String {
    format!("sh -c {}", shell_quote(script.as_ref()))
}

fn list_field_separator(nonce: &str) -> String {
    format!("__CON_TMUX_FIELD_{nonce}__")
}

pub fn build_tmux_list_command(nonce: &str) -> String {
    let sep = list_field_separator(nonce);
    let pane_format = format!(
        "__CON_TMUX__{sep}#{{session_name}}{sep}#{{window_id}}{sep}#{{window_index}}{sep}#{{window_name}}{sep}#{{pane_id}}{sep}#{{pane_index}}{sep}#{{pane_active}}{sep}#{{window_active}}{sep}#{{pane_current_command}}{sep}#{{pane_current_path}}"
    );
    shell_wrap_script(format!(
        r#"
printf "%s\n" "__CON_TMUX_BEGIN_{nonce}__"
tmux list-panes -a -F {} 2>/dev/null
printf "%s\n" "__CON_TMUX_END_{nonce}__"
        "#,
        shell_quote(&pane_format),
    ))
}

pub fn parse_tmux_list_lines(lines: &[String], nonce: &str) -> Result<TmuxSnapshot, String> {
    let begin = format!("__CON_TMUX_BEGIN_{nonce}__");
    let end = format!("__CON_TMUX_END_{nonce}__");
    let field_sep = list_field_separator(nonce);

    let end_idx = lines
        .iter()
        .rposition(|line| line.trim_end() == end)
        .ok_or_else(|| "tmux list end marker not found in pane output".to_string())?;
    let begin_idx = lines[..end_idx]
        .iter()
        .rposition(|line| line.trim_end() == begin)
        .ok_or_else(|| "tmux list begin marker not found in pane output".to_string())?;

    let mut panes = Vec::new();
    for line in &lines[begin_idx + 1..end_idx] {
        let trimmed = line.trim_end_matches('\r');
        let fields = if let Some(rest) = trimmed.strip_prefix(PREFIX) {
            if let Some(rest) = rest.strip_prefix(&field_sep) {
                rest.split(&field_sep)
                    .map(str::to_string)
                    .collect::<Vec<_>>()
            } else {
                let normalized;
                let row = if trimmed.contains("\\t") && !trimmed.contains('\t') {
                    normalized = trimmed.replace("\\t", "\t");
                    normalized.as_str()
                } else {
                    trimmed
                };
                let mut parts = row.splitn(11, '\t');
                if parts.next() != Some(PREFIX) {
                    continue;
                }
                parts.map(str::to_string).collect::<Vec<_>>()
            }
        } else {
            continue;
        };
        if fields.len() != 10 {
            continue;
        }
        let window_target = format!("{}:{}", fields[0], fields[2]);
        let target = format!("{}.{}", window_target, fields[5]);
        panes.push(TmuxPaneInfo {
            session_name: fields[0].clone(),
            window_id: fields[1].clone(),
            window_index: fields[2].clone(),
            window_name: fields[3].clone(),
            window_target,
            pane_id: fields[4].clone(),
            pane_index: fields[5].clone(),
            target,
            pane_active: fields[6] == "1",
            window_active: fields[7] == "1",
            pane_current_command: optional_field(&fields[8]),
            pane_current_path: optional_field(&fields[9]),
        });
    }

    if panes.is_empty() {
        return Err("tmux list markers were present but no pane rows were parsed".to_string());
    }

    Ok(TmuxSnapshot { panes })
}

pub fn build_tmux_capture_command(nonce: &str, target: Option<&str>, lines: usize) -> String {
    let target_flag = target
        .map(|value| format!("-t {} ", shell_quote(value)))
        .unwrap_or_default();
    shell_wrap_script(format!(
        r#"
printf "%s\n" "__CON_TMUX_CAPTURE_BEGIN_{nonce}__"
tmux capture-pane -p -J {target_flag}-S -{lines} 2>/dev/null
printf "%s\n" "__CON_TMUX_CAPTURE_END_{nonce}__"
        "#
    ))
}

pub fn parse_tmux_capture_lines(
    lines: &[String],
    nonce: &str,
    target: Option<&str>,
) -> Result<TmuxCapture, String> {
    let begin = format!("__CON_TMUX_CAPTURE_BEGIN_{nonce}__");
    let end = format!("__CON_TMUX_CAPTURE_END_{nonce}__");

    let end_idx = lines
        .iter()
        .rposition(|line| line.contains(&end))
        .ok_or_else(|| "tmux capture end marker not found in pane output".to_string())?;
    let begin_idx = lines[..end_idx]
        .iter()
        .rposition(|line| line.contains(&begin))
        .ok_or_else(|| "tmux capture begin marker not found in pane output".to_string())?;

    let content = lines[begin_idx + 1..end_idx].join("\n");
    Ok(TmuxCapture {
        target: target.unwrap_or("current").to_string(),
        content,
    })
}

pub fn build_tmux_send_keys_command(
    target: &str,
    literal_text: Option<&str>,
    key_names: &[String],
    append_enter: bool,
) -> Result<String, String> {
    if literal_text.is_none() && key_names.is_empty() {
        return Err("tmux_send_keys requires literal_text or key_names".to_string());
    }

    let mut commands = Vec::new();
    if let Some(text) = literal_text {
        commands.push(format!(
            "tmux send-keys -t {} -l -- {}",
            shell_quote(target),
            shell_quote(text)
        ));
    }
    if !key_names.is_empty() {
        let joined = key_names
            .iter()
            .map(|key| shell_quote(key))
            .collect::<Vec<_>>()
            .join(" ");
        commands.push(format!(
            "tmux send-keys -t {} {}",
            shell_quote(target),
            joined
        ));
    }
    if append_enter {
        commands.push(format!("tmux send-keys -t {} Enter", shell_quote(target)));
    }

    Ok(format!("sh -c {}", shell_quote(&commands.join(" && "))))
}

pub fn build_tmux_exec_command(
    nonce: &str,
    location: TmuxExecLocation,
    target: Option<&str>,
    command: &str,
    window_name: Option<&str>,
    cwd: Option<&str>,
    detached: bool,
) -> String {
    let mut args = vec![
        "tmux".to_string(),
        match location {
            TmuxExecLocation::NewWindow => "new-window".to_string(),
            TmuxExecLocation::SplitHorizontal | TmuxExecLocation::SplitVertical => {
                "split-window".to_string()
            }
        },
        "-P".to_string(),
        "-F".to_string(),
        shell_quote(&format!(
            "__CON_TMUX_EXEC__{sep}#{{session_name}}{sep}#{{window_id}}{sep}#{{window_index}}{sep}#{{window_name}}{sep}#{{pane_id}}{sep}#{{pane_index}}",
            sep = list_field_separator(nonce),
        )),
    ];

    if detached {
        args.push("-d".to_string());
    }
    if let Some(target) = target {
        args.push("-t".to_string());
        args.push(shell_quote(target));
    }
    match location {
        TmuxExecLocation::SplitHorizontal => args.push("-h".to_string()),
        TmuxExecLocation::SplitVertical => args.push("-v".to_string()),
        TmuxExecLocation::NewWindow => {}
    }
    if let Some(window_name) = window_name
        && matches!(location, TmuxExecLocation::NewWindow)
    {
        args.push("-n".to_string());
        args.push(shell_quote(window_name));
    }
    if let Some(cwd) = cwd {
        args.push("-c".to_string());
        args.push(shell_quote(cwd));
    }
    args.push(shell_quote(command));

    shell_wrap_script(format!(
        r#"
printf "%s\n" "__CON_TMUX_EXEC_BEGIN_{nonce}__"
{}
printf "%s\n" "__CON_TMUX_EXEC_END_{nonce}__"
        "#,
        args.join(" ")
    ))
}

pub fn parse_tmux_exec_lines(
    lines: &[String],
    nonce: &str,
    location: TmuxExecLocation,
    detached: bool,
) -> Result<TmuxExecResult, String> {
    let begin = format!("__CON_TMUX_EXEC_BEGIN_{nonce}__");
    let end = format!("__CON_TMUX_EXEC_END_{nonce}__");
    let separator = list_field_separator(nonce);

    let end_idx = lines
        .iter()
        .rposition(|line| line.contains(&end))
        .ok_or_else(|| "tmux exec end marker not found in pane output".to_string())?;
    let begin_idx = lines[..end_idx]
        .iter()
        .rposition(|line| line.contains(&begin))
        .ok_or_else(|| "tmux exec begin marker not found in pane output".to_string())?;

    for line in &lines[begin_idx + 1..end_idx] {
        let trimmed = line.trim_end_matches('\r');
        if !trimmed.starts_with("__CON_TMUX_EXEC__") {
            continue;
        }
        let fields = if trimmed.contains(&separator) {
            trimmed
                .split(&separator)
                .skip(1)
                .map(str::to_string)
                .collect::<Vec<_>>()
        } else {
            trimmed
                .split('\t')
                .skip(1)
                .map(str::to_string)
                .collect::<Vec<_>>()
        };
        if fields.len() != 6 {
            continue;
        }
        let window_target = format!("{}:{}", fields[0], fields[2]);
        let target = format!("{}.{}", window_target, fields[5]);
        return Ok(TmuxExecResult {
            location,
            detached,
            session_name: fields[0].to_string(),
            window_id: fields[1].to_string(),
            window_index: fields[2].to_string(),
            window_name: fields[3].to_string(),
            window_target,
            pane_id: fields[4].to_string(),
            pane_index: fields[5].to_string(),
            target,
        });
    }

    Err("tmux exec markers were present but no target row was parsed".to_string())
}

fn optional_field(value: &str) -> Option<String> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::{
        TmuxExecLocation, build_tmux_capture_command, build_tmux_exec_command,
        build_tmux_list_command, build_tmux_send_keys_command, parse_tmux_capture_lines,
        parse_tmux_exec_lines, parse_tmux_list_lines,
    };

    #[test]
    fn list_parser_extracts_tmux_panes() {
        let lines = vec![
            "__CON_TMUX_BEGIN_x__".to_string(),
            "__CON_TMUX__\twork\t@1\t1\tshell\t%3\t0\t1\t1\tzsh\t/home/w".to_string(),
            "__CON_TMUX__\twork\t@2\t2\tcodex\t%7\t1\t0\t0\tcodex\t/home/w/repo".to_string(),
            "__CON_TMUX_END_x__".to_string(),
        ];
        let snapshot = parse_tmux_list_lines(&lines, "x").expect("parse");
        assert_eq!(snapshot.panes.len(), 2);
        assert_eq!(snapshot.panes[1].pane_id, "%7");
        assert_eq!(snapshot.panes[1].window_index, "2");
        assert_eq!(snapshot.panes[1].target, "work:2.1");
        assert_eq!(
            snapshot.panes[1].pane_current_command.as_deref(),
            Some("codex")
        );
    }

    #[test]
    fn list_parser_extracts_nonce_scoped_separator_rows() {
        let lines = vec![
            "__CON_TMUX_BEGIN_x__".to_string(),
            "__CON_TMUX____CON_TMUX_FIELD_x__work__CON_TMUX_FIELD_x__@1__CON_TMUX_FIELD_x__1__CON_TMUX_FIELD_x__shell__CON_TMUX_FIELD_x__%3__CON_TMUX_FIELD_x__0__CON_TMUX_FIELD_x__1__CON_TMUX_FIELD_x__1__CON_TMUX_FIELD_x__zsh__CON_TMUX_FIELD_x__/home/w".to_string(),
            "__CON_TMUX_END_x__".to_string(),
        ];
        let snapshot = parse_tmux_list_lines(&lines, "x").expect("parse");
        assert_eq!(snapshot.panes.len(), 1);
        assert_eq!(snapshot.panes[0].window_target, "work:1");
    }

    #[test]
    fn list_parser_accepts_literal_backslash_t_rows() {
        let lines = vec![
            "__CON_TMUX_BEGIN_x__".to_string(),
            "__CON_TMUX__\\twork\\t@1\\t1\\tshell\\t%3\\t0\\t1\\t1\\tzsh\\t/home/w".to_string(),
            "__CON_TMUX_END_x__".to_string(),
        ];
        let snapshot = parse_tmux_list_lines(&lines, "x").expect("parse");
        assert_eq!(snapshot.panes.len(), 1);
        assert_eq!(snapshot.panes[0].target, "work:1.0");
    }

    #[test]
    fn capture_parser_extracts_content() {
        let lines = vec![
            "prompt".to_string(),
            "__CON_TMUX_CAPTURE_BEGIN_x__".to_string(),
            "hello".to_string(),
            "world".to_string(),
            "__CON_TMUX_CAPTURE_END_x__".to_string(),
        ];
        let capture = parse_tmux_capture_lines(&lines, "x", Some("%7")).expect("capture");
        assert_eq!(capture.target, "%7");
        assert_eq!(capture.content, "hello\nworld");
    }

    #[test]
    fn capture_parser_accepts_markers_with_prompt_prefix() {
        let lines = vec![
            "❯ __CON_TMUX_CAPTURE_BEGIN_x__".to_string(),
            "hello".to_string(),
            "__CON_TMUX_CAPTURE_END_x__".to_string(),
        ];
        let capture = parse_tmux_capture_lines(&lines, "x", Some("%7")).expect("capture");
        assert_eq!(capture.content, "hello");
    }

    #[test]
    fn builders_include_markers() {
        let list = build_tmux_list_command("n");
        assert!(list.contains("__CON_TMUX_BEGIN_n__"));
        assert!(list.contains("__CON_TMUX_FIELD_n__"));
        assert!(
            build_tmux_capture_command("n", Some("%3"), 80)
                .contains("__CON_TMUX_CAPTURE_BEGIN_n__")
        );
        assert!(
            build_tmux_exec_command(
                "n",
                TmuxExecLocation::NewWindow,
                None,
                "htop",
                Some("monitor"),
                Some("/tmp"),
                true,
            )
            .contains("__CON_TMUX_EXEC_BEGIN_n__")
        );
        let cmd = build_tmux_send_keys_command("%3", Some("ls"), &[], true).expect("cmd");
        assert!(cmd.contains("tmux send-keys"));
        assert!(cmd.contains("Enter"));
    }

    #[test]
    fn exec_parser_extracts_spawned_target() {
        let lines = vec![
            "__CON_TMUX_EXEC_BEGIN_x__".to_string(),
            "__CON_TMUX_EXEC____CON_TMUX_FIELD_x__work__CON_TMUX_FIELD_x__@9__CON_TMUX_FIELD_x__9__CON_TMUX_FIELD_x__monitor__CON_TMUX_FIELD_x__%31__CON_TMUX_FIELD_x__0".to_string(),
            "__CON_TMUX_EXEC_END_x__".to_string(),
        ];
        let exec =
            parse_tmux_exec_lines(&lines, "x", TmuxExecLocation::NewWindow, true).expect("exec");
        assert_eq!(exec.window_target, "work:9");
        assert_eq!(exec.target, "work:9.0");
        assert!(exec.detached);
    }

    #[test]
    fn exec_parser_accepts_wrapped_marker_lines() {
        let lines = vec![
            "❯ __CON_TMUX_EXEC_BEGIN_x__".to_string(),
            "__CON_TMUX_EXEC____CON_TMUX_FIELD_x__work__CON_TMUX_FIELD_x__@9__CON_TMUX_FIELD_x__9__CON_TMUX_FIELD_x__monitor__CON_TMUX_FIELD_x__%31__CON_TMUX_FIELD_x__0".to_string(),
            "__CON_TMUX_EXEC_END_x__".to_string(),
        ];
        let exec =
            parse_tmux_exec_lines(&lines, "x", TmuxExecLocation::NewWindow, true).expect("exec");
        assert_eq!(exec.target, "work:9.0");
    }

    #[test]
    fn exec_parser_accepts_legacy_tab_rows() {
        let lines = vec![
            "__CON_TMUX_EXEC_BEGIN_x__".to_string(),
            "__CON_TMUX_EXEC__\twork\t@9\t9\tmonitor\t%31\t0".to_string(),
            "__CON_TMUX_EXEC_END_x__".to_string(),
        ];
        let exec =
            parse_tmux_exec_lines(&lines, "x", TmuxExecLocation::NewWindow, true).expect("exec");
        assert_eq!(exec.target, "work:9.0");
    }
}
