//! The editor with real files in temp folders (no cluster, no file watcher).

use std::path::{Path, PathBuf};
use std::time::Duration;

use gpui::{Entity, TestAppContext, VisualTestContext};
use kubyl_settings::Settings;

use crate::editor::KubeconfigEditor;
use crate::files::{self, SaveError};
use crate::model::{self, EntryRef, Kind};
use crate::settings::KubeconfigSettings;
use crate::state::{Dirs, Kubeconfigs};

const FILE: &str = "# team kubeconfig, keep this comment\napiVersion: v1\nkind: Config\nclusters:\n- name: dev # local\n  cluster:\n    server: https://127.0.0.1:6443\ncontexts:\n- name: dev\n  context:\n    cluster: dev\n    user: dev\nusers:\n- name: dev\n  user:\n    token: abc\ncurrent-context: dev\n";

struct Setup {
    _dir: tempfile::TempDir,
    owned: PathBuf,
    backups: PathBuf,
    external: PathBuf,
}

fn setup(cx: &mut TestAppContext) -> Setup {
    let dir = tempfile::tempdir().unwrap();
    let owned = dir.path().join("config/kubeconfigs");
    let backups = dir.path().join("config/kubeconfig-backups");
    let external = dir.path().join("home/.kube");
    std::fs::create_dir_all(&owned).unwrap();
    std::fs::create_dir_all(&external).unwrap();
    let config = dir.path().join("config");
    let dirs = Dirs {
        owned: owned.clone(),
        backups: backups.clone(),
    };
    cx.update(|cx| {
        kubyl_core::init(cx);
        kubyl_settings::init_with_dir(cx, &config);
        kubyl_ui::init(cx);
        gpui_component::init(cx);
        Settings::register::<KubeconfigSettings>(cx);
        Kubeconfigs::install_with(dirs, cx);
    });
    Setup {
        owned,
        backups,
        external,
        _dir: dir,
    }
}

fn open<'a>(
    path: &Path,
    cx: &'a mut TestAppContext,
) -> (Entity<KubeconfigEditor>, &'a mut VisualTestContext) {
    let path = path.to_path_buf();
    let (editor, cx) =
        cx.add_window_view(move |window, cx| KubeconfigEditor::new(path.clone(), window, cx));
    cx.run_until_parked();
    (editor, cx)
}

fn set_namespace(editor: &Entity<KubeconfigEditor>, ns: &str, cx: &mut VisualTestContext) {
    let ns = ns.to_string();
    editor.update_in(cx, |editor, window, cx| {
        editor.edit(window, cx, |doc| {
            model::set_str(
                doc.body_mut(Kind::Context, "dev").unwrap(),
                &["namespace"],
                &ns,
            );
        });
    });
}

async fn save(
    editor: &Entity<KubeconfigEditor>,
    opt_in: bool,
    cx: &mut VisualTestContext,
) -> Result<files::Saved, SaveError> {
    let task = editor.update_in(cx, |editor, window, cx| {
        let text = editor.text_to_save().text;
        editor.save_file(text, opt_in, window, cx)
    });
    let result = task.await;
    cx.run_until_parked();
    result
}

#[gpui::test]
async fn saves_one_changed_line_with_a_backup(cx: &mut TestAppContext) {
    let s = setup(cx);
    let path = s.owned.join("team.yaml");
    std::fs::write(&path, FILE).unwrap();
    let (editor, cx) = open(&path, cx);
    editor.read_with(cx, |editor, _| {
        assert!(editor.load_error.is_none(), "{:?}", editor.load_error);
        assert_eq!(editor.selection, Some(EntryRef::new(Kind::Context, "dev")));
        assert!(!editor.is_dirty());
        assert!(editor.is_owned());
    });
    set_namespace(&editor, "payments", cx);
    editor.read_with(cx, |editor, _| {
        assert!(editor.is_dirty());
        assert_eq!(editor.change_count, 1);
    });
    let saved = save(&editor, false, cx).await.unwrap();
    let text = std::fs::read_to_string(&path).unwrap();
    assert_eq!(
        text,
        FILE.replace(
            "    user: dev\nusers:",
            "    user: dev\n    namespace: payments\nusers:"
        )
    );
    assert!(text.contains("# team kubeconfig, keep this comment") && text.contains("# local"));
    let backup = saved.backup.expect("a backup");
    assert!(backup.starts_with(&s.backups));
    assert_eq!(std::fs::read_to_string(backup).unwrap(), FILE);
    #[cfg(unix)]
    assert_eq!(saved.mode, Some(0o600), "inline token: 0600");
    editor.read_with(cx, |editor, _| assert!(!editor.is_dirty()));
}

#[gpui::test]
async fn a_file_changed_on_disk_shows_the_banner_and_is_not_overwritten_blindly(
    cx: &mut TestAppContext,
) {
    let s = setup(cx);
    let path = s.owned.join("team.yaml");
    std::fs::write(&path, FILE).unwrap();
    let (editor, cx) = open(&path, cx);

    // Without local changes, a change on disk is simply taken.
    std::fs::write(&path, FILE.replace("abc", "def")).unwrap();
    cx.executor().advance_clock(Duration::from_secs(3));
    cx.run_until_parked();
    editor.read_with(cx, |editor, _| {
        assert!(editor.disk.is_none());
        let token = model::get_str(editor.doc.body(Kind::User, "dev").unwrap(), &["token"]);
        assert_eq!(token, "def");
    });

    // With local changes: the banner, and a save is refused.
    set_namespace(&editor, "payments", cx);
    let theirs = FILE
        .replace("abc", "def")
        .replace("# local", "# edited by aws-cli");
    std::fs::write(&path, &theirs).unwrap();
    cx.executor().advance_clock(Duration::from_secs(3));
    cx.run_until_parked();
    editor.read_with(cx, |editor, _| {
        assert!(editor.disk.is_some(), "the banner shows");
        assert!(editor.is_dirty());
    });
    assert!(matches!(
        save(&editor, false, cx).await,
        Err(SaveError::Changed { .. })
    ));
    assert_eq!(std::fs::read_to_string(&path).unwrap(), theirs);

    // "Keep mine": the save writes the edit onto the new file, keeping its comment.
    editor.update(cx, |editor, cx| editor.keep_mine(cx));
    save(&editor, false, cx).await.unwrap();
    let text = std::fs::read_to_string(&path).unwrap();
    assert!(
        text.contains("namespace: payments") && text.contains("# edited by aws-cli"),
        "{text}"
    );
}

#[gpui::test]
async fn files_kubyl_does_not_own_need_an_opt_in(cx: &mut TestAppContext) {
    let s = setup(cx);
    let path = s.external.join("config");
    std::fs::write(&path, FILE).unwrap();
    let (editor, cx) = open(&path, cx);
    editor.read_with(cx, |editor, cx| {
        assert!(!editor.is_owned());
        assert!(!editor.is_editable(cx));
    });
    set_namespace(&editor, "payments", cx);
    assert!(matches!(
        save(&editor, false, cx).await,
        Err(SaveError::Io(_))
    ));
    assert_eq!(std::fs::read_to_string(&path).unwrap(), FILE);
    save(&editor, true, cx).await.unwrap();
    assert!(
        std::fs::read_to_string(&path)
            .unwrap()
            .contains("namespace: payments")
    );
    editor.read_with(cx, |editor, cx| {
        assert!(editor.is_editable(cx));
        assert!(Settings::get::<KubeconfigSettings>(cx).opted_in(&path));
    });
}

#[gpui::test]
fn renames_and_the_yaml_tab_stay_in_sync(cx: &mut TestAppContext) {
    let s = setup(cx);
    let path = s.owned.join("team.yaml");
    std::fs::write(&path, FILE).unwrap();
    let (editor, cx) = open(&path, cx);
    editor.update_in(cx, |editor, window, cx| {
        editor.rename(
            &EntryRef::new(Kind::Cluster, "dev"),
            "kind".into(),
            window,
            cx,
        );
        assert_eq!(editor.doc.context_refs("dev").0.as_deref(), Some("kind"));
        editor.set_tab(crate::editor::Tab::Yaml, window, cx);
    });
    cx.run_until_parked();
    // The YAML tab shows the file as it will be written, with the token masked.
    let text = editor.read_with(cx, |editor, cx| {
        editor.yaml.editor.read(cx).value().to_string()
    });
    assert!(text.contains("- name: kind # local"), "{text}");
    assert!(text.contains("cluster: kind"));
    assert!(
        !text.contains("abc") && text.contains(crate::yaml_tab::MASK),
        "{text}"
    );
    // Typing in the YAML tab updates the document; the masked token stays what it was.
    let edited = text.replace(
        "current-context: dev",
        "current-context: dev\npreferences: {}",
    );
    editor.update_in(cx, |editor, window, cx| {
        editor
            .yaml
            .editor
            .update(cx, |state, cx| state.set_value(edited, window, cx));
    });
    editor.update_in(cx, |editor, window, cx| {
        editor.yaml_typed_for_test(window, cx)
    });
    editor.read_with(cx, |editor, _| {
        assert!(editor.doc.0.get("preferences").is_some());
        assert_eq!(
            model::get_str(editor.doc.body(Kind::User, "dev").unwrap(), &["token"]),
            "abc"
        );
    });
}
