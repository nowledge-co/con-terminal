//! Open macOS folder documents as new terminals, independently of workspace profiles.

use std::path::PathBuf;

use con_core::{Config, session::Session};
use gpui::{App, Application, Global};
use tokio::sync::mpsc::{self, UnboundedReceiver};
use url::Url;

use crate::startup_args::StartupArgs;

pub(crate) struct FolderRequests(Option<UnboundedReceiver<Vec<String>>>);
impl Global for FolderRequests {}

pub(crate) fn register(app: &Application) -> FolderRequests {
    let (sender, receiver) = mpsc::unbounded_channel();
    app.on_open_urls(move |urls| {
        // AppKit can deliver documents before GPUI startup or while the
        // first-run import prompt is open. Queue without borrowing the app.
        let _ = sender.send(urls);
    });
    FolderRequests(Some(receiver))
}

fn folder_paths(urls: Vec<String>) -> impl Iterator<Item = PathBuf> {
    urls.into_iter().filter_map(|raw| {
        let url = Url::parse(&raw).ok()?;
        if url.scheme() != "file" || url.host().is_some() {
            return None;
        }
        let path = url.to_file_path().ok()?;
        // macOS's terminal surface takes a UTF-8 working directory.
        path.to_str()?;
        path.is_dir().then_some(path)
    })
}

pub(crate) fn launch(config: Config, startup: StartupArgs, cx: &mut App) {
    let mut requests = cx
        .global_mut::<FolderRequests>()
        .0
        .take()
        .expect("folder request listener starts once");
    let mut initial_folders = Vec::new();
    while let Ok(urls) = requests.try_recv() {
        initial_folders.extend(folder_paths(urls));
    }
    // A folder is sufficient to start a fresh app. If there is a previous
    // session or an explicit startup request, still honor the normal launch.
    if initial_folders.is_empty()
        || startup != StartupArgs::default()
        || Session::session_path().exists()
    {
        crate::open_con_window_with_startup(
            config.clone(),
            crate::startup_session(&startup),
            true,
            Some(startup),
            cx,
        );
    }
    open_windows(initial_folders, &config, cx);

    cx.spawn(async move |cx| {
        while let Some(urls) = requests.recv().await {
            let folders: Vec<_> = folder_paths(urls).collect();
            if !folders.is_empty() {
                cx.update(|cx| {
                    if let Some(config) = crate::load_config_for_new_window(cx) {
                        open_windows(folders, &config, cx);
                    }
                });
            }
        }
    })
    .detach();
}

fn open_windows(folders: Vec<PathBuf>, config: &Config, cx: &mut App) {
    for folder in folders {
        let session = crate::fresh_window_session_with_history_for_cwd(Some(folder));
        crate::open_con_window(config.clone(), session, false, cx);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_local_directories_with_escaped_paths_and_ignores_other_items() {
        let temp = tempfile::tempdir().unwrap();
        let folder = temp.path().join("a b 中文 #%25");
        std::fs::create_dir(&folder).unwrap();
        let file = folder.join("regular file");
        std::fs::write(&file, "").unwrap();
        let folder_url = Url::from_directory_path(&folder).unwrap();
        let missing_url = Url::from_directory_path(folder.join("missing")).unwrap();
        let remote_url = format!("file://example.com{}", folder_url.path());

        assert_eq!(
            folder_paths(vec![
                remote_url,
                "https://example.com/".into(),
                Url::from_file_path(file).unwrap().into(),
                missing_url.into(),
                folder_url.into(),
            ])
            .collect::<Vec<_>>(),
            vec![folder],
        );
    }
}
