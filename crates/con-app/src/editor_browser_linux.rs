//! Launch the HTTPS handler explicitly; a file-association fallback could open an editor.

use std::io;

#[cfg(target_os = "linux")]
pub(super) fn open(url: &url::Url) -> io::Result<()> {
    open_with(url, |program, args| {
        con_paths::host_command(program)
            .args(args)
            .output()
            .map_err(|error| {
                io::Error::new(error.kind(), format!("Could not run {program}: {error}"))
            })
    })
}

fn open_with(
    url: &url::Url,
    mut run: impl FnMut(&str, &[&str]) -> io::Result<std::process::Output>,
) -> io::Result<()> {
    let query = run("xdg-mime", &["query", "default", "x-scheme-handler/https"])?;
    if !query.status.success() {
        return Err(io::Error::other("Could not find the default web browser"));
    }
    let desktop_id = String::from_utf8(query.stdout)
        .map_err(|_| io::Error::other("Invalid default browser desktop entry"))?;
    let desktop_id = desktop_id.trim();
    if desktop_id.is_empty()
        || !desktop_id.ends_with(".desktop")
        || desktop_id.contains(['/', '\\', '\n', '\r'])
    {
        return Err(io::Error::other("Invalid default browser desktop entry"));
    }

    // gtk-launch resolves desktop IDs (including nested entries) on the same
    // host as the query. Never search the sandbox's application directories.
    // Pass the escaped file URL as a single argument; never interpolate a shell command.
    let launch = run("gtk-launch", &["--", desktop_id, url.as_str()])?;
    if launch.status.success() {
        Ok(())
    } else {
        Err(io::Error::other("Could not launch the default web browser"))
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::open_with;
    use std::{os::unix::process::ExitStatusExt, process::Output};

    fn output(code: i32, stdout: &str) -> Output {
        Output {
            status: std::process::ExitStatus::from_raw(code << 8),
            stdout: stdout.as_bytes().to_vec(),
            stderr: Vec::new(),
        }
    }

    #[test]
    fn launches_browser_desktop_id_with_one_escaped_file_uri_argument() {
        let url = url::Url::parse("file:///tmp/HTML%20%23preview.html").unwrap();
        let mut calls = Vec::new();
        open_with(&url, |program, args| {
            calls.push((
                program.to_string(),
                args.iter().map(|arg| arg.to_string()).collect::<Vec<_>>(),
            ));
            Ok(if program == "xdg-mime" {
                output(0, "foo-bar.desktop\n")
            } else {
                output(0, "")
            })
        })
        .unwrap();
        assert_eq!(
            calls,
            vec![
                (
                    "xdg-mime".into(),
                    vec![
                        "query".into(),
                        "default".into(),
                        "x-scheme-handler/https".into()
                    ]
                ),
                (
                    "gtk-launch".into(),
                    vec!["--".into(), "foo-bar.desktop".into(), url.to_string()]
                ),
            ]
        );
    }

    #[test]
    fn missing_or_invalid_browser_never_falls_back_to_html_file_association() {
        let url = url::Url::parse("file:///tmp/index.html").unwrap();
        for (code, entry) in [
            (1, "firefox.desktop"),
            (0, ""),
            (0, "/tmp/browser.desktop"),
            (0, "firefox.desktop\neditor.desktop"),
        ] {
            let mut calls = 0;
            let result = open_with(&url, |program, _| {
                assert_eq!(program, "xdg-mime");
                calls += 1;
                Ok(output(code, entry))
            });
            assert!(result.is_err());
            assert_eq!(calls, 1);
        }
    }

    #[test]
    fn browser_launch_failure_is_reported_without_trying_another_application() {
        let url = url::Url::parse("file:///tmp/index.html").unwrap();
        let mut calls = Vec::new();
        let result = open_with(&url, |program, _| {
            calls.push(program.to_string());
            Ok(if program == "xdg-mime" {
                output(0, "firefox.desktop")
            } else {
                output(1, "")
            })
        });
        assert!(result.is_err());
        assert_eq!(calls, ["xdg-mime", "gtk-launch"]);
    }
}
