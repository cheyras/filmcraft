//! Source clip effects through the commands: `effects.*` and `lumetri.*` take `item` (a media
//! item or a subclip of one) beside `clip`; each is one undoable edit; the render shows the
//! effects as the same effects on the clip; and hostile parameters are errors, never panics or
//! half-made edits.

use serde_json::{Value, json};

use crate::Session;
use filmcraft_project::{ClipId, ItemId, ItemKind, Label, ParamValue};
use filmcraft_time::{TICKS_PER_SECOND, Tick, TimeRange};

fn demo() -> Session {
    let mut s = Session::default();
    s.execute("file.openDemoProject", json!({})).unwrap();
    s
}

/// The first V1 clip under the playhead, selected, and its media item.
fn pick(s: &mut Session) -> (ClipId, ItemId) {
    let q = s.active_sequence().unwrap();
    let it = q.video_tracks[0].item_at(s.playhead()).unwrap();
    let (clip, item) = (it.id, it.item);
    s.state.selection = vec![clip];
    (clip, item)
}

fn source_fx(s: &Session, item: ItemId) -> Vec<filmcraft_project::EffectInstance> {
    s.project.source_effects_of(item).to_vec()
}

fn undo_steps(s: &Session) -> usize {
    s.history.undo.len()
}

fn picture(s: &Session) -> filmcraft_render::Image {
    s.render_program(0.125).unwrap()
}

fn worst(a: &filmcraft_render::Image, b: &filmcraft_render::Image) -> f32 {
    assert_eq!((a.w, a.h), (b.w, b.h));
    a.px.iter().zip(&b.px).map(|(x, y)| (x - y).abs()).fold(0.0, f32::max)
}

fn param(s: &Session, item: ItemId, index: usize, id: &str) -> filmcraft_project::Param {
    source_fx(s, item)[index].params[id].clone()
}

#[test]
fn apply_list_inspect_undo_and_redo() {
    let mut s = demo();
    let (_, item) = pick(&mut s);
    let steps = undo_steps(&s);
    let r = s.execute("effects.apply", json!({"item": item.0, "effect": "gaussian_blur"})).unwrap();
    assert_eq!((r["items"][0].as_u64(), r["effect"].as_str()), (Some(item.0), Some("gaussian_blur")));
    s.execute("effects.apply", json!({"items": [item.0], "effect": "Lumetri Color"})).unwrap();
    assert_eq!(undo_steps(&s), steps + 2, "one undo step each");
    let list = s.execute("effects.list", json!({"item": item.0})).unwrap();
    assert_eq!(list["item"], json!(item.0));
    let fx = list["effects"].as_array().unwrap();
    assert_eq!(fx.iter().map(|e| e["effect"].as_str().unwrap()).collect::<Vec<_>>(), ["gaussian_blur", "lumetri"], "in render order");
    assert!(fx.iter().all(|e| e["enabled"] == true && e["allowed"] == true));
    assert_eq!(fx[1]["index"], 1);
    // the catalogue listing is as before
    assert!(s.execute("effects.list", json!({"kind": "Video"})).unwrap().as_array().is_some_and(|a| a.len() > 20));
    // the project and sequence inspections count them
    let proj = s.execute("project.inspect", json!({})).unwrap();
    assert!(proj.to_string().contains(r#""sourceEffects":2"#), "{proj}");
    let seq = s.execute("sequence.inspect", json!({})).unwrap();
    let clips: Vec<&Value> = seq["video"][0]["items"].as_array().unwrap().iter().filter(|c| c["item"] == json!(item.0)).collect();
    assert!(!clips.is_empty() && clips.iter().all(|c| c["sourceEffects"] == 2), "{clips:?}");
    // undo and redo
    s.execute("edit.undo", json!({})).unwrap();
    assert_eq!(source_fx(&s, item).len(), 1);
    s.execute("edit.undo", json!({})).unwrap();
    assert!(source_fx(&s, item).is_empty());
    s.execute("edit.redo", json!({})).unwrap();
    s.execute("edit.redo", json!({})).unwrap();
    assert_eq!(source_fx(&s, item).iter().map(|e| e.effect.as_str()).collect::<Vec<_>>(), ["gaussian_blur", "lumetri"]);
    assert!(s.is_dirty());
}

#[test]
fn a_source_effect_renders_as_the_same_effect_on_the_clip() {
    let mut s = demo();
    let (clip, item) = pick(&mut s);
    let plain = picture(&s);
    // on the media
    s.execute("effects.apply", json!({"item": item.0, "effect": "lumetri"})).unwrap();
    s.execute("effects.setParam", json!({"item": item.0, "effect": "lumetri", "param": "exposure", "value": 1.0})).unwrap();
    s.execute("effects.setParam", json!({"item": item.0, "effect": 0, "param": "saturation", "value": 40.0})).unwrap();
    let on_media = picture(&s);
    assert!(worst(&on_media, &plain) > 0.02);
    // the same on the clip, with the media's removed
    s.execute("effects.remove", json!({"item": item.0, "index": 0})).unwrap();
    assert_eq!(worst(&picture(&s), &plain), 0.0);
    s.execute("effects.apply", json!({"clips": [clip.0], "effect": "lumetri"})).unwrap();
    let idx = s.active_sequence().unwrap().find_item(clip).unwrap().1.effects.iter().position(|e| e.effect == "lumetri").unwrap();
    s.execute("effects.setParam", json!({"clip": clip.0, "effect": idx, "param": "exposure", "value": 1.0})).unwrap();
    s.execute("effects.setParam", json!({"clip": clip.0, "effect": idx, "param": "saturation", "value": 40.0})).unwrap();
    assert_eq!(worst(&picture(&s), &on_media), 0.0);
    // the Source Monitor shows the media's effects, too
    s.execute("effects.remove", json!({"clip": clip.0, "index": idx})).unwrap();
    s.execute("effects.apply", json!({"item": item.0, "effect": "lumetri"})).unwrap();
    s.execute("effects.setParam", json!({"item": item.0, "effect": "lumetri", "param": "exposure", "value": 1.0})).unwrap();
    let provider = s.media.provider(s.project.clone(), s.services.clone());
    let shown = filmcraft_render::render_item(&s.project, item, Tick::ZERO, 0.125, &provider).unwrap();
    s.execute("effects.toggleEnabled", json!({"item": item.0, "index": 0})).unwrap();
    let off = filmcraft_render::render_item(&s.project, item, Tick::ZERO, 0.125, &provider).unwrap();
    assert!(worst(&shown, &off) > 0.02, "the monitor follows the toggle");
}

#[test]
fn parameters_and_keyframes_are_in_media_time() {
    let mut s = demo();
    let (_, item) = pick(&mut s);
    s.execute("effects.apply", json!({"item": item.0, "effect": "lumetri"})).unwrap();
    let at = |sec: i64| sec * TICKS_PER_SECOND;
    let base = undo_steps(&s);
    // a keyframe at a media time, and a second; each a step
    for (sec, v) in [(2, 0.0), (4, 2.0)] {
        s.execute("effects.setParam", json!({"item": item.0, "effect": "lumetri", "param": "exposure", "value": v, "keyframe": true, "time": at(sec)}))
            .unwrap();
    }
    assert_eq!(undo_steps(&s), base + 2);
    let times: Vec<i64> = param(&s, item, 0, "exposure").keyframes.iter().map(|k| k.time.0).collect();
    assert_eq!(times, [at(2), at(4)], "media ticks, kept as given");
    assert_eq!(param(&s, item, 0, "exposure").f64_at(Tick(at(3))), 1.0);
    // the listing shows them
    let list = s.execute("effects.list", json!({"item": item.0})).unwrap();
    assert_eq!(list["effects"][0]["params"]["exposure"]["keyframes"], json!([at(2), at(4)]));
    // add (a toggle at `time`), edit, move, interpolate, delete: by media time
    s.execute("effects.addKeyframe", json!({"item": item.0, "effect": "lumetri", "param": "exposure", "time": at(3)})).unwrap();
    assert_eq!(param(&s, item, 0, "exposure").keyframes.len(), 3);
    s.execute("effects.addKeyframe", json!({"item": item.0, "effect": "lumetri", "param": "exposure", "time": at(3)})).unwrap();
    assert_eq!(param(&s, item, 0, "exposure").keyframes.len(), 2, "a second add at the same time removes it");
    s.execute("effects.setKeyframe", json!({"item": item.0, "effect": "lumetri", "param": "exposure", "mediaTime": at(4), "value": 3.0})).unwrap();
    assert_eq!(param(&s, item, 0, "exposure").f64_at(Tick(at(4))), 3.0);
    s.execute("effects.moveKeyframe", json!({"item": item.0, "effect": "lumetri", "param": "exposure", "mediaTime": at(4), "to": at(5)})).unwrap();
    assert_eq!(param(&s, item, 0, "exposure").keyframes.iter().map(|k| k.time.0).collect::<Vec<_>>(), [at(2), at(5)]);
    s.execute("effects.setInterpolation", json!({"item": item.0, "effect": "lumetri", "param": "exposure", "mediaTime": at(2), "interpolation": "hold"}))
        .unwrap();
    assert_eq!(param(&s, item, 0, "exposure").keyframes[0].interp, filmcraft_project::Interpolation::Hold);
    s.execute("effects.deleteKeyframe", json!({"item": item.0, "effect": "lumetri", "param": "exposure", "mediaTime": at(5)})).unwrap();
    assert_eq!(param(&s, item, 0, "exposure").keyframes.len(), 1);
    // the animation toggle, reset and reset parameter, one step each
    let steps = undo_steps(&s);
    s.execute("effects.toggleAnimation", json!({"item": item.0, "effect": "lumetri", "param": "exposure", "time": at(2)})).unwrap();
    assert!(!param(&s, item, 0, "exposure").is_animated());
    s.execute("effects.setParam", json!({"item": item.0, "effect": "lumetri", "param": "exposure", "value": 1.5})).unwrap();
    s.execute("effects.resetParam", json!({"item": item.0, "effect": "lumetri", "param": "exposure"})).unwrap();
    assert_eq!(param(&s, item, 0, "exposure").value, ParamValue::Float(0.0));
    s.execute("effects.setParam", json!({"item": item.0, "effect": "lumetri", "param": "saturation", "value": 10.0})).unwrap();
    s.execute("effects.reset", json!({"item": item.0, "index": 0})).unwrap();
    assert_eq!(param(&s, item, 0, "saturation").value, ParamValue::Float(100.0));
    assert_eq!(undo_steps(&s), steps + 5);
    // a drag is one step, as on a clip
    let steps = undo_steps(&s);
    for v in [0.1, 0.2, 0.3] {
        s.execute("effects.setParam", json!({"item": item.0, "effect": "lumetri", "param": "exposure", "value": v, "merge": true})).unwrap();
    }
    assert_eq!(undo_steps(&s), steps + 1);
    s.execute("edit.undo", json!({})).unwrap();
    assert_eq!(param(&s, item, 0, "exposure").value, ParamValue::Float(0.0));
}

#[test]
fn a_source_keyframe_without_a_time_uses_the_source_monitor() {
    let mut s = demo();
    let (_, item) = pick(&mut s);
    s.execute("effects.apply", json!({"item": item.0, "effect": "lumetri"})).unwrap();
    // not open in the Source Monitor: media time 0
    s.execute("effects.addKeyframe", json!({"item": item.0, "effect": "lumetri", "param": "exposure"})).unwrap();
    assert_eq!(param(&s, item, 0, "exposure").keyframes[0].time, Tick::ZERO);
    // open there, its playhead is the time
    s.state.source_item = Some(item);
    s.state.source_playhead = Tick(3 * TICKS_PER_SECOND);
    s.execute("effects.addKeyframe", json!({"item": item.0, "effect": "lumetri", "param": "exposure"})).unwrap();
    assert_eq!(param(&s, item, 0, "exposure").keyframes.iter().map(|k| k.time.0).collect::<Vec<_>>(), [0, 3 * TICKS_PER_SECOND]);
}

#[test]
fn lumetri_commands_edit_the_source_effects() {
    let mut s = demo();
    let (clip, item) = pick(&mut s);
    let plain = picture(&s);
    let steps = undo_steps(&s);
    // a Lumetri is added with the setting, in one undo step
    let r = s.execute("lumetri.setInputLut", json!({"item": item.0, "lut": "builtin:slog3-sgamut3cine-to-rec709"})).unwrap();
    assert_eq!((r["item"].as_u64(), r["lut"].as_str()), (Some(item.0), Some("builtin:slog3-sgamut3cine-to-rec709")));
    assert_eq!(undo_steps(&s), steps + 1);
    assert_eq!(source_fx(&s, item).len(), 1);
    assert_eq!(param(&s, item, 0, "input_lut").value, ParamValue::Text("builtin:slog3-sgamut3cine-to-rec709".into()));
    assert!(s.active_sequence().unwrap().find_item(clip).unwrap().1.effects.iter().all(|e| e.effect != "lumetri"), "the clip got nothing");
    let graded = picture(&s);
    assert!(worst(&graded, &plain) > 0.02);
    s.execute("lumetri.setLook", json!({"item": item.0, "lut": "builtin:look-monochrome"})).unwrap();
    assert_eq!(source_fx(&s, item).len(), 1, "the same Lumetri");
    assert_eq!(param(&s, item, 0, "look_lut").value, ParamValue::Text("builtin:look-monochrome".into()));
    // a section switch toggles; on: sets
    s.execute("lumetri.setSection", json!({"item": item.0, "section": "basic"})).unwrap();
    assert_eq!(param(&s, item, 0, "basic_on").value, ParamValue::Bool(false));
    s.execute("lumetri.setSection", json!({"item": item.0, "section": "basic", "on": true})).unwrap();
    assert_eq!(param(&s, item, 0, "basic_on").value, ParamValue::Bool(true));
    // an LUT file: imported and set as one step
    let path = std::env::temp_dir().join(format!("filmcraft-src-{}.cube", std::process::id()));
    std::fs::write(&path, filmcraft_color::Lut::from_cube(filmcraft_color::Lut3d::from_fn(9, |c| [c[2], c[1], c[0]])).to_cube()).unwrap();
    let steps = undo_steps(&s);
    let r = s.execute("lumetri.setInputLut", json!({"item": item.0, "path": path.to_string_lossy()})).unwrap();
    assert!(r["lut"].as_str().unwrap().starts_with("lib:") && s.project.luts.len() == 1);
    assert_eq!(undo_steps(&s), steps + 1, "import and set are one edit");
    s.execute("edit.undo", json!({})).unwrap();
    assert!(s.project.luts.is_empty() && param(&s, item, 0, "input_lut").value == ParamValue::Text("builtin:slog3-sgamut3cine-to-rec709".into()));
    let _ = std::fs::remove_file(&path);
    // clearing
    s.execute("lumetri.setInputLut", json!({"item": item.0, "lut": ""})).unwrap();
    assert_eq!(param(&s, item, 0, "input_lut").value, ParamValue::Text(String::new()));
    // a preset becomes another source effect
    let steps = undo_steps(&s);
    let r = s.execute("lumetri.applyPreset", json!({"item": item.0, "name": "Neutral Mono"})).unwrap();
    assert_eq!(r["items"], json!([item.0]));
    assert_eq!(undo_steps(&s), steps + 1);
    assert_eq!(source_fx(&s, item).len(), 2);
    // Apply Match is a timeline-clip operation
    let e = s.execute("lumetri.applyMatch", json!({"item": item.0, "referenceTime": 0})).unwrap_err().to_string();
    assert!(e.contains("unsupported for source clips"), "{e}");
    // undo all the way
    while undo_steps(&s) > 0 && !source_fx(&s, item).is_empty() {
        s.execute("edit.undo", json!({})).unwrap();
    }
    assert_eq!(worst(&picture(&s), &plain), 0.0);
}

#[test]
fn toggle_and_remove_by_index() {
    let mut s = demo();
    let (_, item) = pick(&mut s);
    for e in ["invert", "gaussian_blur", "lumetri"] {
        s.execute("effects.apply", json!({"item": item.0, "effect": e})).unwrap();
    }
    s.execute("effects.toggleEnabled", json!({"item": item.0, "index": 1})).unwrap();
    assert_eq!(source_fx(&s, item).iter().map(|e| e.enabled).collect::<Vec<_>>(), [true, false, true]);
    s.execute("effects.remove", json!({"item": item.0, "index": 0})).unwrap();
    assert_eq!(source_fx(&s, item).iter().map(|e| e.effect.as_str()).collect::<Vec<_>>(), ["gaussian_blur", "lumetri"]);
    // an index past the end changes nothing (as on a clip)
    s.execute("effects.remove", json!({"item": item.0, "index": 9})).unwrap();
    s.execute("effects.toggleEnabled", json!({"item": item.0, "index": 9})).unwrap();
    assert_eq!(source_fx(&s, item).len(), 2);
}

#[test]
fn a_subclip_edits_the_media_it_comes_from() {
    let mut s = demo();
    let (_, item) = pick(&mut s);
    let sub = {
        let p = std::sync::Arc::make_mut(&mut s.project);
        p.add_item(
            "sub",
            Label::Iris,
            ItemKind::Subclip { parent: item, range: TimeRange::new(Tick::ZERO, Tick(2 * TICKS_PER_SECOND)), restrict_trims: false },
            None,
        )
    };
    let r = s.execute("effects.apply", json!({"item": sub.0, "effect": "lumetri"})).unwrap();
    assert_eq!(r["items"], json!([item.0]), "resolved to the media");
    assert_eq!(source_fx(&s, item).len(), 1);
    assert_eq!(s.project.source_effects_of(sub).len(), 1);
    s.execute("effects.setParam", json!({"item": sub.0, "effect": "lumetri", "param": "exposure", "value": 0.5})).unwrap();
    assert_eq!(param(&s, item, 0, "exposure").value, ParamValue::Float(0.5));
    assert_eq!(s.execute("effects.list", json!({"item": sub.0})).unwrap()["item"], json!(item.0));
    // both ids name the same media: applied once
    s.execute("effects.apply", json!({"items": [item.0, sub.0], "effect": "invert"})).unwrap();
    assert_eq!(source_fx(&s, item).len(), 2);
}

/// A command that must fail without changing anything and without a panic.
fn refused(s: &mut Session, cmd: &str, params: Value) -> String {
    let (steps, rev, fx) = (undo_steps(s), s.revision, serde_json::to_string(&s.project.items).unwrap());
    let e = s.execute(cmd, params.clone()).expect_err(&format!("{cmd} {params} should be refused")).to_string();
    assert!(!e.contains("internal error"), "{cmd} {params}: a panic: {e}");
    assert_eq!((undo_steps(s), s.revision), (steps, rev), "{cmd} {params}: nothing was edited");
    assert_eq!(serde_json::to_string(&s.project.items).unwrap(), fx, "{cmd} {params}: the project is as it was");
    e
}

#[test]
fn hostile_item_parameters_are_errors_that_change_nothing() {
    let mut s = demo();
    let (clip, item) = pick(&mut s);
    s.execute("effects.apply", json!({"item": item.0, "effect": "lumetri"})).unwrap();
    let seq = s.state.active_sequence.unwrap();
    let bin = s.project.root.id.0;
    let bad_items = [
        json!("x"),
        json!(""),
        json!(-1),
        json!(1.5),
        json!(f64::MAX),
        json!(true),
        json!([item.0]),
        json!({"a": 1}),
        json!(0),
        json!(u64::MAX),
        json!(987654321),
        json!(seq.0),
        json!(bin),
    ];
    let commands: Vec<(&str, Value)> = vec![
        ("effects.apply", json!({"effect": "invert"})),
        ("effects.remove", json!({"index": 0})),
        ("effects.toggleEnabled", json!({"index": 0})),
        ("effects.setParam", json!({"effect": "lumetri", "param": "exposure", "value": 1.0})),
        ("effects.toggleAnimation", json!({"effect": "lumetri", "param": "exposure"})),
        ("effects.reset", json!({"index": 0})),
        ("effects.resetParam", json!({"effect": "lumetri", "param": "exposure"})),
        ("effects.addKeyframe", json!({"effect": "lumetri", "param": "exposure"})),
        ("effects.deleteKeyframe", json!({"effect": "lumetri", "param": "exposure", "mediaTime": 0})),
        ("effects.moveKeyframe", json!({"effect": "lumetri", "param": "exposure", "mediaTime": 0, "to": 5})),
        ("effects.setKeyframe", json!({"effect": "lumetri", "param": "exposure", "mediaTime": 0})),
        ("effects.setInterpolation", json!({"effect": "lumetri", "param": "exposure", "mediaTime": 0, "interpolation": "hold"})),
        ("lumetri.setInputLut", json!({"lut": ""})),
        ("lumetri.setLook", json!({"lut": ""})),
        ("lumetri.setSection", json!({"section": "basic"})),
        ("lumetri.applyPreset", json!({"name": "Neutral Mono"})),
        ("lumetri.applyMatch", json!({"referenceTime": 0})),
    ];
    for (cmd, params) in &commands {
        for item_v in &bad_items {
            let mut p = params.clone();
            p["item"] = item_v.clone();
            refused(&mut s, cmd, p);
        }
        // `clip` together with `item` (or `items`) is an error whichever is valid
        let mut p = params.clone();
        p["item"] = json!(item.0);
        p["clip"] = json!(clip.0);
        let e = refused(&mut s, cmd, p);
        assert!(e.contains("not both") || e.contains("unsupported"), "{cmd}: {e}");
        let mut p = params.clone();
        p["items"] = json!([item.0]);
        p["clips"] = json!([clip.0]);
        refused(&mut s, cmd, p);
    }
    // wrong types for the other parameters of an item that is fine
    let ok = json!(item.0);
    for (cmd, p) in [
        ("effects.apply", json!({"item": ok, "effect": 5})),
        ("effects.apply", json!({"item": ok, "effect": "no such effect"})),
        ("effects.apply", json!({"item": ok})),
        ("effects.apply", json!({"items": [], "effect": "invert"})),
        ("effects.apply", json!({"items": "x", "effect": "invert"})),
        ("effects.apply", json!({"items": [item.0, "x"], "effect": "invert"})),
        ("effects.apply", json!({"items": [item.0, seq.0], "effect": "invert"})),
        ("effects.apply", json!({"item": ok, "items": [item.0], "effect": "invert"})),
        ("effects.remove", json!({"item": ok})),
        ("effects.remove", json!({"item": ok, "index": "x"})),
        ("effects.setParam", json!({"item": ok, "effect": "lumetri", "param": "exposure"})),
        ("effects.setParam", json!({"item": ok, "effect": "lumetri", "param": "exposure", "value": "loud"})),
        ("effects.setParam", json!({"item": ok, "effect": "lumetri", "param": "nope", "value": 1})),
        ("effects.setParam", json!({"item": ok, "effect": "nope", "param": "exposure", "value": 1})),
        ("effects.setParam", json!({"item": ok, "effect": 99, "param": "exposure", "value": 1})),
        ("effects.setParam", json!({"item": ok, "effect": [1], "param": "exposure", "value": 1})),
        ("effects.setParam", json!({"item": ok, "effect": "lumetri", "param": 3, "value": 1})),
        ("effects.addKeyframe", json!({"item": ok, "effect": "lumetri", "param": "nope"})),
        ("effects.addKeyframe", json!({"item": ok, "effect": "nope", "param": "exposure"})),
        ("effects.deleteKeyframe", json!({"item": ok, "effect": "lumetri", "param": "exposure"})),
        ("effects.moveKeyframe", json!({"item": ok, "effect": "lumetri", "param": "exposure", "mediaTime": 0})),
        ("effects.setKeyframe", json!({"item": ok, "effect": "lumetri", "param": "exposure", "mediaTime": 12345})),
        ("effects.setInterpolation", json!({"item": ok, "effect": "lumetri", "param": "exposure", "mediaTime": 12345})),
        ("effects.resetParam", json!({"item": ok, "effect": "lumetri", "param": "nope"})),
        ("effects.list", json!({"item": 7777777})),
        ("effects.list", json!({"item": "x"})),
        ("lumetri.setInputLut", json!({"item": ok, "lut": "lib:nope"})),
        ("lumetri.setInputLut", json!({"item": ok, "lut": "builtin:nope"})),
        ("lumetri.setInputLut", json!({"item": ok, "path": "/no/such/file.cube"})),
        ("lumetri.setLook", json!({"item": ok, "lut": 5})),
        ("lumetri.setSection", json!({"item": ok, "section": "nope"})),
        ("lumetri.setSection", json!({"item": ok})),
        ("lumetri.applyPreset", json!({"item": ok, "name": "No Such Look"})),
        ("lumetri.applyPreset", json!({"items": [], "name": "Neutral Mono"})),
        ("lumetri.applyPreset", json!({"items": [seq.0], "name": "Neutral Mono"})),
    ] {
        refused(&mut s, cmd, p);
    }
}

#[test]
fn effects_a_source_cannot_have_are_refused() {
    let mut s = demo();
    let (_, item) = pick(&mut s);
    for id in [
        "motion",
        "opacity",
        "time_remap",
        "volume",
        "parametric_eq",
        "cross_dissolve",
        "dip_to_black",
        "echo",
        "posterize_time",
        "warp_stabilizer",
        "auto_reframe",
        "track_matte",
        "compound_blur",
        "timecode",
        "graphic_text",
        "graphic_shape",
    ] {
        let e = refused(&mut s, "effects.apply", json!({"item": item.0, "effect": id}));
        assert!(e.contains("source clip effect") || e.contains("graphic layers"), "{id}: {e}");
        refused(&mut s, "effects.apply", json!({"items": [item.0], "effect": id}));
    }
    // a preset is a Lumetri: fine; the same effects on a clip are unaffected by the refusal
    let (clip, _) = pick(&mut s);
    s.execute("effects.apply", json!({"clips": [clip.0], "effect": "echo"})).unwrap();
    assert!(source_fx(&s, item).is_empty());
}

#[test]
fn media_without_a_picture_takes_no_source_effects() {
    let mut s = demo();
    let (_, item) = pick(&mut s);
    let audio = {
        let p = std::sync::Arc::make_mut(&mut s.project);
        let mut m = p.item(item).unwrap().as_media().unwrap().clone();
        m.info.video = None;
        p.add_item("sound", Label::Iris, ItemKind::Media(m), None)
    };
    let e = refused(&mut s, "effects.apply", json!({"item": audio.0, "effect": "invert"}));
    assert!(e.contains("no picture"), "{e}");
    refused(&mut s, "effects.list", json!({"item": audio.0}));
}

#[test]
fn timeline_clip_commands_are_as_before() {
    let mut s = demo();
    let (clip, _) = pick(&mut s);
    // no target at all, and the selection fallbacks of the commands that had one
    let e = refused(&mut s, "effects.remove", json!({"index": 0}));
    assert!(e.contains("`clip`") && e.contains("`item`"), "{e}");
    let n = s.active_sequence().unwrap().find_item(clip).unwrap().1.effects.len();
    s.execute("effects.apply", json!({"effect": "invert"})).unwrap();
    assert_eq!(s.active_sequence().unwrap().find_item(clip).unwrap().1.effects.len(), n + 1, "the selected clip");
    s.execute("lumetri.setInputLut", json!({"lut": "builtin:slog3-sgamut3cine-to-rec709"})).unwrap();
    assert!(s.active_sequence().unwrap().find_item(clip).unwrap().1.effects.iter().any(|e| e.effect == "lumetri"));
    // the parameter listing mentions both targets
    let spec = crate::command_specs().iter().find(|c| c.id == "effects.setParam").unwrap();
    assert!(spec.params.contains("\"item\":id"), "{}", spec.params);
}

/// Known limitation, kept as a failing-on-purpose test (`cargo test -- --ignored`): Create Proxies
/// renders the media through the whole pipeline (`proxies::transcode` clones the `MediaClip`, source
/// effects and Interpret Footage included), so the proxy file already has the source effects in its
/// pixels; playing with proxies on then runs them again over the proxy frames. The same happens to
/// Interpret Footage's colour-space override today. The fix is to transcode the media without its
/// source effects and interpretation; until then a proxy of media with source effects looks wrong.
#[test]
#[ignore = "known limitation: a proxy has the source effects baked in, and playback applies them again"]
fn a_proxy_shows_the_picture_of_the_full_resolution_media() {
    use crate::media_test_util::{frame_rgba, make_movie, psnr, session_with, tmp_dir};
    let root = tmp_dir("proxy-source-effects");
    let a = root.join("a.mov");
    make_movie(&a, filmcraft_media::DemoScene::OceanSunset, 320, 180, 12);
    let (mut s, items, _) = session_with(&[&a]);
    // Invert twice is the original picture: a doubled effect is plain to see
    s.execute("effects.apply", json!({"item": items[0].0, "effect": "invert"})).unwrap();
    let full = frame_rgba(&mut s, 5, 1.0);
    s.execute("media.createProxies", json!({"items": [items[0].0], "preset": "prores_proxy_half", "wait": true})).unwrap();
    s.execute("media.toggleProxies", json!({"enabled": true})).unwrap();
    let proxied = frame_rgba(&mut s, 5, 1.0);
    let q = psnr(&proxied.2, &full.2);
    let _ = std::fs::remove_dir_all(&root);
    assert!(q > 25.0, "the proxy picture matches the full-resolution one: {q:.1} dB");
}

#[test]
fn extreme_media_times_neither_panic_nor_break_the_render() {
    let mut s = demo();
    let (_, item) = pick(&mut s);
    s.execute("effects.apply", json!({"item": item.0, "effect": "lumetri"})).unwrap();
    let plain = picture(&s);
    for t in [i64::MIN, i64::MIN + 1, -1, 0, 1, i64::MAX - 1, i64::MAX] {
        for cmd in ["effects.setParam", "effects.addKeyframe"] {
            let r = s.execute(cmd, json!({"item": item.0, "effect": "lumetri", "param": "exposure", "value": 1.0, "keyframe": true, "time": t}));
            if let Err(e) = r {
                assert!(!e.to_string().contains("internal error"), "{cmd} at {t}: {e}");
            }
        }
        // whatever was written, the picture renders and the commands read it back
        let img = picture(&s);
        assert_eq!((img.w, img.h), (plain.w, plain.h));
        assert!(img.px.iter().all(|v| v.is_finite()), "finite pixels at {t}");
        for cmd in ["effects.deleteKeyframe", "effects.moveKeyframe", "effects.setKeyframe"] {
            let r = s.execute(cmd, json!({"item": item.0, "effect": "lumetri", "param": "exposure", "mediaTime": t, "to": 0, "value": 2.0}));
            if let Err(e) = r {
                assert!(!e.to_string().contains("internal error"), "{cmd} at {t}: {e}");
            }
        }
        s.execute("effects.list", json!({"item": item.0})).unwrap();
    }
    // the project with them still saves and loads
    let bytes = filmcraft_format::encode(&s.project, false);
    assert!(filmcraft_format::decode(&bytes).is_ok());
}
