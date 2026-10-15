//! The Lumetri Color and Effect Controls panels edit either the selected clip or the source clip
//! effects of the media open in the Source Monitor: the pills that pick one, how the pick follows
//! the editor's context, edits landing on the right target through `clip` / `item` commands, the
//! Effect Controls source view, effects dropped on it, and the marks on the Timeline and in the
//! Project panel. Driven headless through the control channel and the harness's pointer.
//!
//! With `FILMCRAFT_UI_SHOTS=<dir>` the last test also renders the UI with wgpu and writes the
//! review PNGs there.

use std::sync::mpsc::{Sender, channel};

use egui_kittest::Harness;
use filmcraft_engine::Session;
use filmcraft_project::{BinEntry, ClipId, ItemId, ItemKind, ParamValue};
use filmcraft_ui_egui::FilmcraftApp;
use filmcraft_ui_egui::control::ControlRequest;
use serde_json::{Value, json};

struct Driver {
    harness: Harness<'static, FilmcraftApp>,
    tx: Sender<ControlRequest>,
    shots: Option<std::path::PathBuf>,
}

/// What the demo project has: a clip on V1, its media, a second media, the sequence's name.
struct Demo {
    clip: ClipId,
    clip_name: String,
    media: ItemId,
    media_name: String,
    other: ItemId,
    sequence: String,
}

impl Driver {
    fn new() -> (Self, Demo) {
        Self::sized(1.0, 1600.0, 980.0)
    }

    fn sized(ppp: f32, w: f32, h: f32) -> (Self, Demo) {
        let mut session = Session::default();
        session.execute("file.openDemoProject", json!({})).expect("demo project");
        let (tx, rx) = channel();
        let app = FilmcraftApp::new(session).with_control(rx);
        let shots = std::env::var_os("FILMCRAFT_UI_SHOTS").map(std::path::PathBuf::from);
        let mut b = Harness::builder().with_size(egui::vec2(w, h)).with_max_steps(10_000);
        if shots.is_some() {
            b = b.wgpu().with_pixels_per_point(ppp);
        }
        let harness = b.build_eframe(move |_cc| app);
        let mut d = Driver { harness, tx, shots };
        d.frames(4);
        let s = &d.harness.state().session;
        let q = s.active_sequence().expect("a sequence");
        let it = q.video_tracks[0].item_at(s.playhead()).expect("a clip under the playhead");
        let media = it.item;
        // another media with a picture
        let other = s
            .project
            .items
            .values()
            .find(|i| i.id != media && matches!(&i.kind, ItemKind::Media(m) if m.info.video.is_some()))
            .map(|i| i.id)
            .expect("a second media");
        let demo = Demo {
            clip: it.id,
            clip_name: it.name.clone(),
            media,
            media_name: s.project.item(media).map(|i| i.name.clone()).unwrap_or_default(),
            other,
            sequence: s.state.active_sequence.and_then(|i| s.project.item(i)).map(|i| i.name.clone()).unwrap_or_default(),
        };
        d.exec("timeline.select", json!({"clips": []}));
        d.frames(3);
        (d, demo)
    }

    fn frames(&mut self, n: usize) {
        for _ in 0..n {
            let ctx = self.harness.ctx.clone();
            let mut raw = std::mem::take(self.harness.input_mut());
            eframe::App::raw_input_hook(self.harness.state_mut(), &ctx, &mut raw);
            *self.harness.input_mut() = raw;
            self.harness.step();
        }
    }
    fn call(&mut self, method: &str, params: Value) -> Value {
        let (req, reply) = ControlRequest::new(method, params.clone());
        self.tx.send(req).unwrap();
        for _ in 0..600 {
            self.frames(1);
            if let Ok(v) = reply.try_recv() {
                return v;
            }
        }
        panic!("no reply to {method} {params}");
    }
    fn ok(&mut self, method: &str, params: Value) -> Value {
        let r = self.call(method, params.clone());
        assert_eq!(r["ok"], true, "{method} {params}: {r}");
        r["result"].clone()
    }
    fn exec(&mut self, command: &str, params: Value) -> Value {
        self.ok("engine.execute", json!({"command": command, "params": params}))
    }
    fn session(&self) -> &Session {
        &self.harness.state().session
    }
    fn show(&mut self, panel: &str) {
        self.ok("ui.panel.show", json!({"panel": panel}));
        self.frames(3);
    }
    /// The element, or None when it is not on screen.
    fn element(&mut self, id: &str) -> Option<Value> {
        self.frames(2);
        let v = self.ok("ui.elements", json!({"prefix": id}));
        v.as_array().unwrap().iter().find(|e| e["id"] == id).cloned()
    }
    fn must(&mut self, id: &str) -> Value {
        self.element(id).unwrap_or_else(|| panic!("no element {id}"))
    }
    fn label(&mut self, id: &str) -> String {
        self.must(id)["label"].as_str().unwrap_or_default().to_string()
    }
    fn enabled(&mut self, id: &str) -> bool {
        self.must(id)["enabled"].as_bool().unwrap_or(true)
    }
    fn rect(&mut self, id: &str) -> egui::Rect {
        let r = self.must(id)["rect"].clone();
        let n = |i: usize| r[i].as_f64().unwrap() as f32;
        egui::Rect::from_min_size(egui::pos2(n(0), n(1)), egui::vec2(n(2), n(3)))
    }
    fn click(&mut self, id: &str) {
        self.must(id);
        self.ok("ui.click", json!({"id": id}));
        self.frames(4);
    }
    fn state(&mut self) -> Value {
        self.ok("ui.inspect", json!({}))
    }
    /// `ui.fx_target.pick`
    fn pick(&mut self) -> String {
        self.frames(2);
        self.state()["ui"]["fx_target"]["pick"].as_str().unwrap().to_string()
    }
    fn status(&mut self) -> String {
        self.state()["ui"]["status"].as_str().unwrap_or_default().to_string()
    }
    fn open_source(&mut self, item: ItemId) {
        self.exec("source.open", json!({"item": item.0}));
        self.frames(3);
    }
    fn source_fx(&self, item: ItemId) -> Vec<filmcraft_project::EffectInstance> {
        self.session().project.source_effects_of(item).to_vec()
    }
    /// The last command of that name the session ran, with its parameters.
    fn last(&self, command: &str) -> Value {
        self.session().journal.iter().rev().find(|(c, _)| c == command).map(|(_, p)| p.clone()).unwrap_or_else(|| panic!("no {command} was run"))
    }
    fn clip_has_lumetri(&self, clip: ClipId) -> bool {
        self.session().active_sequence().unwrap().find_item(clip).unwrap().1.effects.iter().any(|e| e.effect == "lumetri")
    }

    fn pointer(&mut self, pos: egui::Pos2, pressed: Option<bool>) {
        self.harness.input_mut().events.push(egui::Event::PointerMoved(pos));
        if pressed == Some(true) {
            self.frames(1);
        }
        if let Some(pressed) = pressed {
            self.harness.input_mut().events.push(egui::Event::PointerButton {
                pos,
                button: egui::PointerButton::Primary,
                pressed,
                modifiers: egui::Modifiers::NONE,
            });
        }
        self.frames(1);
    }
    /// Press at `from`, drag to `to` in steps, let go.
    fn drag(&mut self, from: egui::Pos2, to: egui::Pos2) {
        self.pointer(from, Some(true));
        for i in 1..=6 {
            self.pointer(from.lerp(to, i as f32 / 6.0), None);
        }
        self.pointer(to, Some(false));
        self.frames(3);
    }

    fn shot(&mut self, name: &str, panel: Option<&str>) {
        let Some(dir) = self.shots.clone() else { return };
        for _ in 0..40 {
            self.frames(1);
            std::thread::sleep(std::time::Duration::from_millis(25));
        }
        // (the pointer is not part of the picture)
        self.harness.input_mut().events.push(egui::Event::PointerGone);
        self.frames(3);
        let crop = panel.map(|p| self.rect(&format!("panel.{p}")));
        let ppp = self.harness.ctx.pixels_per_point();
        let img = self.harness.render().expect("wgpu render");
        std::fs::create_dir_all(&dir).unwrap();
        let img = match crop {
            Some(r) => {
                let (x, y) = ((r.min.x * ppp).max(0.0) as u32, (r.min.y * ppp).max(0.0) as u32);
                let (w, h) = (((r.width() * ppp) as u32).min(img.width().saturating_sub(x)), ((r.height() * ppp) as u32).min(img.height().saturating_sub(y)));
                image::imageops::crop_imm(&img, x, y, w, h).to_image()
            }
            None => img,
        };
        img.save(dir.join(format!("{name}.png"))).unwrap();
    }
}

#[test]
fn pills_name_what_they_edit_and_dim_what_is_missing() {
    let (mut d, demo) = Driver::new();
    for panel in ["LumetriColor", "EffectControls"] {
        d.show(panel);
    }
    for prefix in ["lumetri", "effectControls"] {
        let (source, sequence) = (format!("{prefix}.target.source"), format!("{prefix}.target.sequence"));
        // nothing open, nothing selected: both are dim and say only what they are for
        assert_eq!(d.label(&source), "Source", "{prefix}");
        assert_eq!(d.label(&sequence), demo.sequence, "{prefix}");
        assert!(!d.enabled(&source) && !d.enabled(&sequence), "{prefix}: nothing behind either pill");
    }
    d.open_source(demo.media);
    d.show("EffectControls");
    assert_eq!(d.label("lumetri.target.source"), format!("Source · {}", demo.media_name));
    assert!(d.enabled("lumetri.target.source"));
    assert!(!d.enabled("lumetri.target.sequence"), "no clip is selected");
    d.exec("timeline.select", json!({"clips": [demo.clip.0]}));
    d.frames(2);
    for prefix in ["lumetri", "effectControls"] {
        assert_eq!(d.label(&format!("{prefix}.target.sequence")), format!("{} · {}", demo.sequence, demo.clip_name), "{prefix}");
        assert!(d.enabled(&format!("{prefix}.target.sequence")) && d.enabled(&format!("{prefix}.target.source")), "{prefix}: both have something");
    }
    // closing the media in the Source Monitor dims its pill again
    d.exec("source.close", json!({}));
    d.frames(3);
    assert!(!d.enabled("lumetri.target.source") && d.enabled("lumetri.target.sequence"));
    assert_eq!(d.label("lumetri.target.source"), "Source");
}

#[test]
fn the_target_follows_the_source_monitor_and_the_timeline_selection() {
    let (mut d, demo) = Driver::new();
    d.show("LumetriColor");
    d.show("EffectControls");
    // opening media in the Source Monitor picks Source
    d.open_source(demo.media);
    assert_eq!(d.pick(), "source");
    // selecting a clip in the Timeline picks Sequence
    d.exec("timeline.select", json!({"clips": [demo.clip.0]}));
    assert_eq!(d.pick(), "sequence");
    // focusing the Source Monitor picks Source
    d.ok("ui.set", json!({"focused": "Source"}));
    assert_eq!(d.pick(), "source");
    // a click on a pill holds until the context changes again, from either panel
    d.click("lumetri.target.sequence");
    assert_eq!(d.pick(), "sequence");
    d.frames(10);
    assert_eq!(d.pick(), "sequence", "an explicit pick stays");
    d.show("EffectControls");
    d.click("effectControls.target.source");
    assert_eq!(d.pick(), "source", "choosing in Effect Controls chooses in Lumetri Color too");
    assert!(d.element("lumetri.add").is_some_and(|e| e["label"] == "Add Lumetri Color to source"), "the Lumetri panel shows the source");
    d.click("lumetri.target.sequence");
    assert_eq!(d.pick(), "sequence");
    // a different media opened picks Source again; a different selection, Sequence
    d.open_source(demo.other);
    assert_eq!(d.pick(), "source");
    d.exec("timeline.select", json!({"clips": []}));
    d.exec("timeline.select", json!({"clips": [demo.clip.0]}));
    assert_eq!(d.pick(), "sequence");
    // the control channel can set it
    d.ok("ui.set", json!({"fxTarget": "source"}));
    assert_eq!(d.pick(), "source");
    let e = d.call("ui.set", json!({"fxTarget": "elsewhere"}));
    assert_eq!(e["ok"], false);
}

#[test]
fn editing_the_source_target_sends_item_commands() {
    let (mut d, demo) = Driver::new();
    d.show("LumetriColor");
    d.open_source(demo.media);
    d.exec("timeline.select", json!({"clips": [demo.clip.0]}));
    d.click("lumetri.target.source");
    assert_eq!(d.pick(), "source");
    // the empty state offers to add the Lumetri to the source; the clip gets nothing
    assert_eq!(d.label("lumetri.add"), "Add Lumetri Color to source");
    d.click("lumetri.add");
    let apply = d.last("effects.apply");
    assert_eq!((apply["item"].as_u64(), apply.get("clips"), apply["effect"].as_str()), (Some(demo.media.0), None, Some("lumetri")));
    assert_eq!(d.source_fx(demo.media).iter().map(|e| e.effect.as_str()).collect::<Vec<_>>(), ["lumetri"]);
    assert!(!d.clip_has_lumetri(demo.clip), "the clip's own effects are untouched");
    assert!(d.element("lumetri.add").is_none() && d.element("lumetri.param.saturation").is_some(), "the controls replace the button");
    // a slider
    d.click("lumetri.param.saturation");
    let set = d.last("effects.setParam");
    assert_eq!((set["item"].as_u64(), set.get("clip"), set["param"].as_str(), set["effect"].as_u64()), (Some(demo.media.0), None, Some("saturation"), Some(0)));
    let saturation = d.source_fx(demo.media)[0].param("saturation").unwrap().value.as_f64().unwrap();
    assert!(saturation < 100.0, "the click moved the slider: {saturation}");
    // keyframed values are at the Source Monitor's playhead, in media time
    d.exec("source.setPlayhead", json!({"seconds": 1.0}));
    d.exec("effects.toggleAnimation", json!({"item": demo.media.0, "effect": 0, "param": "exposure"}));
    d.click("lumetri.param.exposure");
    let exposure = d.source_fx(demo.media)[0].param("exposure").unwrap().clone();
    let playhead = d.session().state.source_playhead;
    assert!(playhead.seconds() > 0.9 && playhead.seconds() < 1.1, "the playhead is a second in (to a frame): {playhead:?}");
    assert_eq!(exposure.keyframes.iter().map(|k| k.time).collect::<Vec<_>>(), [playhead], "one keyframe, at the Source Monitor's playhead, in media time");
    // a section switch, the fx bypass and the reset
    let on = d.source_fx(demo.media)[0].param("basic_on").and_then(|p| p.value.as_bool()).unwrap_or(true);
    d.click("lumetri.switch.Basic Correction");
    let section = d.last("lumetri.setSection");
    assert_eq!(
        (section["item"].as_u64(), section.get("clip"), section["section"].as_str(), section["effect"].as_u64()),
        (Some(demo.media.0), None, Some("basic"), Some(0))
    );
    assert_ne!(d.source_fx(demo.media)[0].param("basic_on").and_then(|p| p.value.as_bool()), Some(on));
    d.click("lumetri.fx");
    assert_eq!(d.last("effects.toggleEnabled")["item"].as_u64(), Some(demo.media.0));
    assert!(!d.source_fx(demo.media)[0].enabled);
    d.click("lumetri.reset");
    assert_eq!(d.last("effects.reset")["item"].as_u64(), Some(demo.media.0));
    assert_eq!(d.source_fx(demo.media)[0].param("saturation").unwrap().value, ParamValue::Float(100.0));
    // and the clip target edits the clip, by `clip`
    d.click("lumetri.target.sequence");
    assert_eq!(d.label("lumetri.add"), "Add Lumetri Color to clip");
    d.click("lumetri.add");
    let apply = d.last("effects.apply");
    assert_eq!((apply["clip"].as_u64(), apply.get("item")), (Some(demo.clip.0), None));
    assert!(d.clip_has_lumetri(demo.clip));
    d.click("lumetri.param.saturation");
    let set = d.last("effects.setParam");
    assert_eq!((set["clip"].as_u64(), set.get("item")), (Some(demo.clip.0), None));
    assert_eq!(d.source_fx(demo.media)[0].param("saturation").unwrap().value, ParamValue::Float(100.0), "the source Lumetri is as it was");
}

#[test]
fn the_instance_menu_lists_every_lumetri_of_the_target() {
    let (mut d, demo) = Driver::new();
    d.show("LumetriColor");
    d.open_source(demo.media);
    d.exec("timeline.select", json!({"clips": []}));
    for _ in 0..2 {
        d.exec("effects.apply", json!({"item": demo.media.0, "effect": "lumetri"}));
    }
    d.frames(3);
    assert_eq!(d.pick(), "source");
    assert_eq!(d.label("lumetri.instance"), "Lumetri Color");
    d.click("lumetri.instance");
    assert_eq!(d.label("lumetri.instance.option.0"), "Lumetri Color");
    assert_eq!(d.label("lumetri.instance.option.1"), "Lumetri Color 2");
    assert!(d.element("lumetri.instance.option.2").is_none());
    d.click("lumetri.instance.option.1");
    assert_eq!(d.label("lumetri.instance"), "Lumetri Color 2");
    // the controls edit the second one
    d.click("lumetri.param.saturation");
    let fx = d.source_fx(demo.media);
    assert_eq!(fx[0].param("saturation").unwrap().value, ParamValue::Float(100.0));
    assert!(fx[1].param("saturation").unwrap().value.as_f64().unwrap() < 100.0);
    assert_eq!(d.last("effects.setParam")["effect"].as_u64(), Some(1));
    d.click("lumetri.switch.Basic Correction");
    assert_eq!(d.last("lumetri.setSection")["effect"].as_u64(), Some(1));
    d.click("lumetri.fx");
    assert_eq!(d.source_fx(demo.media).iter().map(|e| e.enabled).collect::<Vec<_>>(), [true, false]);
    // another Lumetri between: the menu keeps pointing at a Lumetri that exists
    d.exec("effects.apply", json!({"item": demo.media.0, "effect": "gaussian_blur"}));
    d.exec("effects.remove", json!({"item": demo.media.0, "index": 2}));
    d.exec("effects.remove", json!({"item": demo.media.0, "index": 1}));
    d.frames(3);
    assert_eq!(d.label("lumetri.instance"), "Lumetri Color", "the second one is gone");
    // a clip with two shows two, and a different target starts at the first
    d.exec("timeline.select", json!({"clips": [demo.clip.0]}));
    for _ in 0..2 {
        d.exec("effects.apply", json!({"clips": [demo.clip.0], "effect": "lumetri"}));
    }
    d.click("lumetri.target.sequence");
    assert_eq!(d.label("lumetri.instance"), "Lumetri Color");
    d.click("lumetri.instance");
    assert!(d.element("lumetri.instance.option.1").is_some());
}

#[test]
fn effect_controls_lists_source_effects_on_a_ruler_over_the_media() {
    let (mut d, demo) = Driver::new();
    d.open_source(demo.media);
    d.show("EffectControls");
    d.exec("effects.apply", json!({"item": demo.media.0, "effect": "lumetri"}));
    d.exec("timeline.select", json!({"clips": [demo.clip.0]}));
    d.click("effectControls.target.source");
    // the source effects only: no Motion, Opacity or Time Remapping
    assert!(d.element("effectControls.effect.lumetri").is_some());
    for fixed in ["motion", "opacity", "time_remapping"] {
        assert!(d.element(&format!("effectControls.effect.{fixed}")).is_none(), "{fixed} is a clip's");
    }
    // the ruler spans the media; a click on it moves the Source Monitor's playhead
    let duration = d.session().project.item(demo.media).unwrap().duration();
    let ruler = d.rect("effectControls.ruler");
    d.ok("ui.click", json!({"x": ruler.center().x, "y": ruler.min.y + 6.0}));
    d.frames(4);
    let at = d.session().state.source_playhead;
    assert!((at.seconds() - duration.seconds() / 2.0).abs() < duration.seconds() * 0.02 + 0.05, "{at:?} is half way into {duration:?}");
    let playhead = d.rect("effectControls.playhead");
    assert!((playhead.center().x - ruler.center().x).abs() < 3.0, "the playhead handle is under the click");
    // the stopwatch animates the parameter at the playhead, in media time
    d.click("effectControls.lumetri.exposure.stopwatch");
    assert_eq!(d.last("effects.toggleAnimation")["item"].as_u64(), Some(demo.media.0));
    let keys: Vec<i64> = d.source_fx(demo.media)[0].param("exposure").unwrap().keyframes.iter().map(|k| k.time.0).collect();
    assert_eq!(keys, [at.0], "a keyframe at the playhead");
    let key = d.rect(&format!("effectControls.lumetri.exposure.keyframe.{}", at.0));
    assert!((key.center().x - playhead.center().x).abs() < 3.0, "the keyframe sits at the playhead on the ruler");
    // dragging it moves it in media time, through the item
    let to = key.center() + egui::vec2(60.0, 0.0);
    d.drag(key.center(), to);
    let moved: Vec<i64> = d.source_fx(demo.media)[0].param("exposure").unwrap().keyframes.iter().map(|k| k.time.0).collect();
    assert!(moved[0] > at.0, "the keyframe moved later: {moved:?} from {at:?}");
    assert_eq!(d.last("effects.moveKeyframes")["item"].as_u64(), Some(demo.media.0));
    // the row's bypass, reset and Clear
    d.click("effectControls.effect.lumetri.enable");
    assert!(!d.source_fx(demo.media)[0].enabled);
    d.click("effectControls.effect.lumetri.reset");
    assert!(
        !d.source_fx(demo.media)[0].param("exposure").unwrap().is_animated() || !d.source_fx(demo.media)[0].param("exposure").unwrap().keyframes.is_empty()
    );
    d.ok("ui.click", json!({"id": "effectControls.effect.lumetri", "button": "right"}));
    d.frames(3);
    d.click("effectControls.effect.lumetri.clear");
    assert_eq!(d.last("effects.remove")["item"].as_u64(), Some(demo.media.0));
    assert!(d.source_fx(demo.media).is_empty());
    assert!(!d.clip_has_lumetri(demo.clip));
}

#[test]
fn an_effect_dropped_on_effect_controls_or_the_source_monitor_becomes_a_source_effect() {
    let (mut d, demo) = Driver::new();
    d.ok("ui.set", json!({"workspace": "Effects", "effectsSearch": "Gaussian Blur"}));
    d.frames(6);
    d.open_source(demo.media);
    d.show("EffectControls");
    d.exec("timeline.select", json!({"clips": [demo.clip.0]}));
    d.click("effectControls.target.source");
    let clip_effects = d.session().active_sequence().unwrap().find_item(demo.clip).unwrap().1.effects.len();
    let from = d.rect("effects.item.gaussian_blur").center();
    let onto = d.rect("effectControls.lane").center();
    d.drag(from, onto);
    assert_eq!(d.source_fx(demo.media).iter().map(|e| e.effect.as_str()).collect::<Vec<_>>(), ["gaussian_blur"]);
    assert_eq!(d.last("effects.apply")["item"].as_u64(), Some(demo.media.0));
    assert_eq!(d.session().active_sequence().unwrap().find_item(demo.clip).unwrap().1.effects.len(), clip_effects, "the clip got nothing");
    // what the engine refuses shows its reason in the status bar
    d.ok("ui.set", json!({"effectsSearch": "Cross Dissolve"}));
    d.frames(4);
    let from = d.rect("effects.item.cross_dissolve").center();
    d.drag(from, onto);
    assert!(d.status().contains("cannot be a source clip effect"), "{}", d.status());
    assert_eq!(d.source_fx(demo.media).len(), 1);
    // onto the Source Monitor
    d.show("Source");
    d.ok("ui.set", json!({"effectsSearch": "Invert"}));
    d.frames(4);
    let from = d.rect("effects.item.invert").center();
    let monitor = d.rect("panel.Source").center();
    d.drag(from, monitor);
    assert_eq!(d.source_fx(demo.media).iter().map(|e| e.effect.as_str()).collect::<Vec<_>>(), ["gaussian_blur", "invert"]);
    // an effect preset sets up a clip: not a source
    let before = d.source_fx(demo.media).len();
    d.ok("ui.set", json!({"effectsSearch": ""}));
    d.frames(3);
    assert_eq!(d.source_fx(demo.media).len(), before);
}

#[test]
fn the_timeline_and_the_project_panel_mark_source_effects() {
    let (mut d, demo) = Driver::new();
    let badge = format!("timeline.clip.{}.fx", demo.clip.0);
    assert!(d.element(&badge).is_none(), "no effects, no badge");
    d.exec("effects.apply", json!({"item": demo.media.0, "effect": "lumetri"}));
    d.frames(3);
    assert_eq!(d.label(&badge), "Source clip effects: 1");
    d.exec("effects.apply", json!({"clips": [demo.clip.0], "effect": "invert"}));
    d.frames(3);
    assert_eq!(d.label(&badge), "Clip effects: 1\nSource clip effects: 1");
    d.exec("effects.remove", json!({"item": demo.media.0, "index": 0}));
    d.frames(3);
    assert_eq!(d.label(&badge), "Clip effects: 1");
    // media with source effects carries a mark in the Project panel (its bins open)
    d.exec("effects.apply", json!({"item": demo.media.0, "effect": "lumetri"}));
    let bins: Vec<u64> = d.session().project.root.children.iter().filter_map(|c| if let BinEntry::Bin(b) = c { Some(b.id.0) } else { None }).collect();
    d.harness.state_mut().ui.expanded_bins.extend(bins);
    d.click("project.view.List");
    d.frames(3);
    let shown: Vec<String> =
        d.ok("ui.elements", json!({"prefix": "project"})).as_array().unwrap().iter().map(|e| e["id"].as_str().unwrap().to_string()).collect();
    let marks: Vec<&String> = shown.iter().filter(|i| i.ends_with(".sourceFx")).collect();
    assert_eq!(marks, [&format!("project.item.{}.sourceFx", demo.media.0)], "only the media that has them; the Project panel shows {shown:?}");
}

/// The review pictures (`FILMCRAFT_UI_SHOTS=<dir> cargo test … review_pictures`).
#[test]
fn review_pictures() {
    if std::env::var_os("FILMCRAFT_UI_SHOTS").is_none() {
        return;
    }
    let (mut d, demo) = Driver::sized(2.0, 1600.0, 980.0);
    d.show("EffectControls");
    d.show("LumetriColor");
    // no target
    d.exec("timeline.select", json!({"clips": []}));
    d.shot("lumetri-no-target", Some("LumetriColor"));
    // the Sequence target, the clip's Lumetri
    d.exec("timeline.select", json!({"clips": [demo.clip.0]}));
    d.exec("lumetri.setInputLut", json!({"clip": demo.clip.0, "lut": "builtin:slog3-sgamut3cine-to-rec709"}));
    d.exec("effects.setParam", json!({"clip": demo.clip.0, "effect": "lumetri", "param": "contrast", "value": 24.0}));
    d.exec("effects.setParam", json!({"clip": demo.clip.0, "effect": "lumetri", "param": "saturation", "value": 120.0}));
    d.click("lumetri.target.sequence");
    d.shot("lumetri-sequence", Some("LumetriColor"));
    // the Source target, a source Lumetri; a long media name truncates
    d.exec("clip.rename", json!({"item": demo.media.0, "name": "A002_C014_0716XK_001_Interview_Chanida_Wide_Master.mov"}));
    d.open_source(demo.media);
    d.exec("lumetri.setInputLut", json!({"item": demo.media.0, "lut": "builtin:slog3-sgamut3cine-to-rec709"}));
    d.exec("effects.setParam", json!({"item": demo.media.0, "effect": "lumetri", "param": "exposure", "value": 0.7}));
    d.exec("effects.setParam", json!({"item": demo.media.0, "effect": "lumetri", "param": "temperature", "value": 18.0}));
    d.exec("effects.apply", json!({"item": demo.media.0, "effect": "lumetri"}));
    d.exec("timeline.select", json!({"clips": [demo.clip.0]}));
    d.click("lumetri.target.source");
    d.shot("lumetri-source", Some("LumetriColor"));
    d.shot("lumetri-source-full", None);
    // the instance menu open
    d.click("lumetri.instance");
    d.shot("lumetri-source-instances", Some("LumetriColor"));
    d.ok("ui.key", json!({"key": "Escape"}));
    // Effect Controls on the source
    d.exec("effects.apply", json!({"item": demo.media.0, "effect": "gaussian_blur"}));
    d.exec("source.setPlayhead", json!({"seconds": 1.5}));
    d.exec("effects.toggleAnimation", json!({"item": demo.media.0, "effect": 0, "param": "exposure"}));
    d.exec("source.setPlayhead", json!({"seconds": 3.0}));
    d.exec("effects.setParam", json!({"item": demo.media.0, "effect": 0, "param": "exposure", "value": -0.5, "keyframe": true}));
    d.show("EffectControls");
    d.click("effectControls.target.source");
    d.shot("effect-controls-source", Some("EffectControls"));
    d.click("effectControls.target.sequence");
    d.shot("effect-controls-sequence", Some("EffectControls"));
    // the timeline badge, in its states: source effects, clip effects, both, only changed fixed effects
    let mut seen = Vec::new();
    let clips: Vec<(ClipId, ItemId)> = d.session().active_sequence().unwrap().video_tracks[0]
        .items
        .iter()
        .filter(|i| {
            let fresh = !seen.contains(&i.item);
            seen.push(i.item);
            fresh
        })
        .map(|i| (i.id, i.item))
        .collect();
    // a clean slate: the demo's clips have effects of their own
    for (clip, _) in &clips {
        loop {
            let standard = d
                .session()
                .active_sequence()
                .unwrap()
                .find_item(*clip)
                .unwrap()
                .1
                .effects
                .iter()
                .position(|e| e.def().is_some_and(|f| !f.intrinsic) && !filmcraft_project::graphic::is_layer(e));
            let Some(index) = standard else { break };
            d.exec("effects.remove", json!({"clip": clip.0, "index": index}));
        }
    }
    while !d.source_fx(demo.media).is_empty() {
        d.exec("effects.remove", json!({"item": demo.media.0, "index": 0}));
    }
    if let Some((_, item)) = clips.first() {
        d.exec("effects.apply", json!({"item": item.0, "effect": "lumetri"}));
    }
    if let Some((clip, _)) = clips.get(1) {
        d.exec("effects.apply", json!({"clips": [clip.0], "effect": "invert"}));
    }
    if let Some((clip, item)) = clips.get(2) {
        d.exec("effects.apply", json!({"clips": [clip.0], "effect": "invert"}));
        d.exec("effects.apply", json!({"item": item.0, "effect": "lumetri"}));
    }
    if let Some((clip, _)) = clips.get(3) {
        d.exec("effects.setParam", json!({"clip": clip.0, "effect": "opacity", "param": "opacity", "value": 60.0}));
    }
    d.exec("timeline.select", json!({"clips": []}));
    d.shot("timeline-badge", Some("Timeline"));
    // the Project panel's mark on media with source effects, in the List and Icon views
    let bins: Vec<u64> = d.session().project.root.children.iter().filter_map(|c| if let BinEntry::Bin(b) = c { Some(b.id.0) } else { None }).collect();
    d.harness.state_mut().ui.expanded_bins.extend(bins);
    d.click("project.view.List");
    d.shot("project-list", Some("Project"));
    d.click("project.view.Icon");
    let footage = d.session().project.root.children.iter().find_map(|c| match c {
        BinEntry::Bin(b) if b.name == "Footage" => Some(b.id.0),
        _ => None,
    });
    if let Some(bin) = footage {
        d.ok("ui.set", json!({"projectPanel": {"bin": bin}}));
        d.frames(4);
    }
    d.shot("project-icons", Some("Project"));
}
