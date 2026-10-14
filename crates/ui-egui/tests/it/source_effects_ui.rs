//! The Source Monitor and bin thumbnails cache frames of a media item by `FilmcraftApp::item_revision`:
//! it must change whenever what the picture is shown through changes (source clip effects and
//! Interpret Footage), and only then.

use filmcraft_engine::Session;
use filmcraft_ui_egui::FilmcraftApp;
use serde_json::json;

fn app() -> (FilmcraftApp, filmcraft_project::ItemId) {
    let mut session = Session::default();
    session.execute("file.openDemoProject", json!({})).expect("demo project");
    let q = session.active_sequence().expect("a sequence");
    let item = q.video_tracks[0].item_at(session.playhead()).expect("a clip under the playhead").item;
    (FilmcraftApp::new(session), item)
}

#[test]
fn item_revision_follows_source_effects_and_interpretation() {
    let (mut app, item) = app();
    let run = |app: &mut FilmcraftApp, cmd: &str, p: serde_json::Value| app.session.execute(cmd, p).unwrap_or_else(|e| panic!("{cmd}: {e}"));
    let none = app.item_revision(item);
    assert_eq!(app.item_revision(item), none, "stable while nothing changes");
    // added
    run(&mut app, "effects.apply", json!({"item": item.0, "effect": "lumetri"}));
    let added = app.item_revision(item);
    assert_ne!(added, none);
    // a parameter changed, keyframed, toggled
    run(&mut app, "effects.setParam", json!({"item": item.0, "effect": "lumetri", "param": "exposure", "value": 1.0}));
    let changed = app.item_revision(item);
    assert_ne!(changed, added);
    run(&mut app, "effects.addKeyframe", json!({"item": item.0, "effect": "lumetri", "param": "exposure", "time": 0}));
    let keyed = app.item_revision(item);
    assert_ne!(keyed, changed);
    run(&mut app, "effects.toggleEnabled", json!({"item": item.0, "index": 0}));
    let toggled = app.item_revision(item);
    assert_ne!(toggled, keyed);
    // a LUT a source effect names
    run(&mut app, "lumetri.setInputLut", json!({"item": item.0, "lut": "builtin:slog3-sgamut3cine-to-rec709"}));
    assert_ne!(app.item_revision(item), toggled);
    // Interpret Footage (this used not to refresh the cached frames)
    let before = app.item_revision(item);
    run(&mut app, "clip.interpretFootage", json!({"items": [item.0], "colorSpace": "slog3-sgamut3cine"}));
    assert_ne!(app.item_revision(item), before);
    // and the original is back when it is all undone
    while app.session.history.can_undo() && app.item_revision(item) != none {
        run(&mut app, "edit.undo", json!({}));
    }
    assert_eq!(app.item_revision(item), none);
    // an edit that does not touch the media leaves the key alone
    run(&mut app, "playhead.set", json!({"seconds": 1.0}));
    assert_eq!(app.item_revision(item), none);
}
