//! The editing session: undo and redo, coalescing, gestures, save, and
//! picking up changes made on disk.

use programs_as_documents::DocumentSession;

fn session(text: &str) -> DocumentSession {
    DocumentSession::new("unsaved.ptl".into(), text.into())
}

#[test]
fn commits_undo_and_redo() {
    let mut s = session("a");
    assert!(!s.can_undo() && !s.dirty());
    assert!(s.commit("b".into(), None));
    assert!(!s.commit("b".into(), None), "no change, no step");
    assert!(s.commit("c".into(), None));
    assert!(s.dirty());
    assert!(s.undo());
    assert_eq!(s.source(), "b");
    assert!(s.undo());
    assert_eq!(s.source(), "a");
    assert!(!s.undo());
    assert!(s.redo() && s.redo());
    assert_eq!(s.source(), "c");
    assert!(!s.redo());
    // A new edit drops the redo stack.
    s.undo();
    s.commit("d".into(), None);
    assert!(!s.can_redo());
}

#[test]
fn keyed_edits_coalesce_and_other_keys_do_not() {
    let mut s = session("0");
    s.commit("1".into(), Some("x"));
    s.commit("2".into(), Some("x"));
    s.commit("3".into(), Some("y"));
    s.commit("4".into(), None);
    s.commit("5".into(), None);
    for want in ["4", "3", "2", "0"] {
        assert!(s.undo());
        assert_eq!(s.source(), want);
    }
    assert!(!s.can_undo());
}

#[test]
fn a_gesture_is_one_step_and_can_be_cancelled() {
    let mut s = session("0");
    s.begin_gesture();
    for t in ["1", "2", "3"] {
        s.commit(t.into(), None);
    }
    assert!(s.can_undo());
    assert!(s.end_gesture());
    assert!(s.undo());
    assert_eq!(s.source(), "0");
    assert!(!s.can_undo());

    s.begin_gesture();
    s.commit("x".into(), None);
    assert!(s.cancel_gesture());
    assert_eq!(s.source(), "0");
    assert!(!s.can_undo());
    // A gesture that changes nothing leaves no step.
    s.begin_gesture();
    assert!(!s.end_gesture());
    assert!(!s.can_undo());
}

#[test]
fn save_and_poll_disk() {
    let dir = std::env::temp_dir().join(format!("pad-session-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("doc.ptl");
    std::fs::write(&path, "let x = 1\n").unwrap();
    let mut s = DocumentSession::read(&path).unwrap();
    assert_eq!(s.source(), "let x = 1\n");
    assert!(!s.poll_disk());

    s.commit("let x = 2\n".into(), None);
    s.save().unwrap();
    assert!(!s.dirty());
    assert_eq!(std::fs::read_to_string(&path).unwrap(), "let x = 2\n");

    // Another program edits the file: with nothing unsaved, it reloads.
    let bump = |text: &str| {
        std::fs::write(&path, text).unwrap();
        let t = std::time::SystemTime::now() + std::time::Duration::from_secs(5);
        std::fs::File::options().write(true).open(&path).unwrap().set_modified(t).unwrap();
    };
    bump("let x = 3\n");
    assert!(s.poll_disk());
    assert_eq!(s.source(), "let x = 3\n");
    assert!(!s.dirty());
    assert!(s.undo(), "the reload is undoable");
    assert_eq!(s.source(), "let x = 2\n");
    s.redo();

    // With unsaved edits, they are kept and the notice says so.
    s.commit("let x = 4\n".into(), None);
    std::thread::sleep(std::time::Duration::from_millis(10));
    std::fs::write(&path, "let x = 5\n").unwrap();
    let t = std::time::SystemTime::now() + std::time::Duration::from_secs(10);
    std::fs::File::options().write(true).open(&path).unwrap().set_modified(t).unwrap();
    assert!(!s.poll_disk());
    assert_eq!(s.source(), "let x = 4\n");
    assert!(s.notice.as_deref().unwrap().contains("unsaved"));
    assert!(s.dirty());
    std::fs::remove_dir_all(&dir).ok();
}
