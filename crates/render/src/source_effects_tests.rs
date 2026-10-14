//! Source clip effects (Premiere's master clip effects, `MediaClip::source_effects`): the media's
//! own effect list runs ahead of every timeline clip's effects, wherever the media is used. It is
//! the same chain ([`crate::clip_effects`]), so a source effect must render exactly as the same
//! effect on the clip, on the CPU path and on the GPU frame plan, and must run before the clip's
//! own effects, Motion and Opacity.

use super::*;
use filmcraft_color::{Lut, Lut3d};
use filmcraft_media::generators::GeneratorSource;
use filmcraft_media::{DemoScene, Generator, MediaSource};
use filmcraft_project::{ClipId, EffectInstance, Label, MediaClip, MediaRef, ProjectLut, SequenceSettings, TrackKind, find_effect};
use filmcraft_time::{FrameRate, TICKS_PER_SECOND, TimeRange};

const RATE: FrameRate = FrameRate::FPS_24;
const SWAP: &str = "lib:swap";
const WARM: &str = "lib:warm";

/// One 320x180 media item (a demo scene) in a 320x180 sequence, and two curved library LUTs:
/// `swap` (red and blue exchanged and compressed) and `warm` (warm and contrasty). Applying them
/// in the other order gives another picture.
struct Rig {
    p: Project,
    map: SourceMap,
    media: ItemId,
    seq: ItemId,
    clip: ClipId,
}

fn library_lut(id: &str, f: impl Fn([f32; 3]) -> [f32; 3]) -> ProjectLut {
    let text = Lut::from_cube(Lut3d::from_fn(17, f)).to_cube();
    ProjectLut { id: id.into(), name: id.into(), source_path: None, format: "cube".into(), text: text.as_str().into() }
}

/// A Lumetri Color with Input LUT `lut`.
fn lumetri_lut(lut: &str) -> EffectInstance {
    let mut e = find_effect("lumetri").unwrap().instance();
    e.params.get_mut("input_lut").unwrap().value = ParamValue::Text(lut.into());
    e
}

/// A Lumetri Color with Basic and Creative settings only (no LUT: the GPU can run it).
fn lumetri_grade() -> EffectInstance {
    let mut e = find_effect("lumetri").unwrap().instance();
    for (id, v) in
        [("exposure", 0.4), ("temperature", 25.0), ("tint", -10.0), ("contrast", 30.0), ("saturation", 130.0), ("vibrance", 20.0), ("vignette_amount", -1.5)]
    {
        e.params.get_mut(id).unwrap().value = ParamValue::Float(v);
    }
    e
}

fn blur() -> EffectInstance {
    let mut e = find_effect("gaussian_blur").unwrap().instance();
    e.params.get_mut("blurriness").unwrap().value = ParamValue::Float(6.0);
    e
}

fn fx(id: &str) -> EffectInstance {
    find_effect(id).unwrap().instance()
}

impl Rig {
    fn new(generator: Generator) -> Rig {
        let mut p = Project::new("source effects");
        let mut map = SourceMap::default();
        let g = GeneratorSource::new(generator.clone(), 320, 180, RATE, Tick(10 * TICKS_PER_SECOND));
        let info = g.info().clone();
        let media = p.add_item(
            "log footage",
            Label::Iris,
            ItemKind::Media(MediaClip {
                media: MediaRef::Generator(generator),
                info,
                interpret: Default::default(),
                mark_in: None,
                mark_out: None,
                markers: vec![],
                offline: false,
                proxy: None,
                identity: None,
                source_effects: vec![],
            }),
            None,
        );
        map.0.insert(media, Arc::new(g) as SharedSource);
        p.luts.push(library_lut("swap", |c| [c[2].powf(0.8), (c[1] * 1.15).min(1.0), 1.0 - c[0] * 0.5]));
        p.luts.push(library_lut("warm", |c| [(c[0] * 1.2).min(1.0).powf(1.4), c[1].powf(1.1), c[2] * 0.7]));
        let seq = p.new_sequence("s", SequenceSettings { width: 320, height: 180, frame_rate: RATE, ..Default::default() }, 2, 1, None);
        let mut r = Rig { p, map, media, seq, clip: ClipId(0) };
        r.clip = r.put(seq, media);
        r
    }

    fn ocean() -> Rig {
        Rig::new(Generator::Demo(DemoScene::OceanSunset))
    }

    fn put(&mut self, seq: ItemId, item: ItemId) -> ClipId {
        let rate = self.p.sequence(seq).map_or(RATE, |s| s.settings.frame_rate);
        let ti = self.p.make_track_item(item, TrackKind::Video, Tick::ZERO, TimeRange::new(Tick::ZERO, rate.tick_of(48)), rate).unwrap();
        let id = ti.id;
        self.p.sequence_mut(seq).unwrap().video_tracks[0].items.push(ti);
        id
    }

    fn media_mut(&mut self) -> &mut MediaClip {
        match self.p.item_mut(self.media).map(|i| &mut i.kind) {
            Some(ItemKind::Media(m)) => m,
            _ => unreachable!("the rig's media"),
        }
    }

    /// The media's source clip effects.
    fn source(mut self, effects: Vec<EffectInstance>) -> Rig {
        self.media_mut().source_effects = effects;
        self
    }

    /// The clip's own effects, before its fixed ones.
    fn on_clip(mut self, effects: Vec<EffectInstance>) -> Rig {
        let (seq, clip) = (self.seq, self.clip);
        self.clip_mut(seq, clip).effects.splice(0..0, effects);
        self
    }

    fn clip_mut(&mut self, seq: ItemId, clip: ClipId) -> &mut TrackItem {
        self.p.sequence_mut(seq).unwrap().find_item_mut(clip).unwrap().1
    }

    fn render(&self, t: Tick, opts: RenderOptions) -> Image {
        render_sequence(&self.p, self.seq, t, opts, &self.map)
    }

    /// The same frame through the frame plan the GPU compositor is given, run on the CPU.
    fn planned(&self, t: Tick, opts: RenderOptions) -> Image {
        plan::execute_cpu(&plan::plan_frame(&self.p, self.seq, t, opts, &self.map))
    }

    /// Whether the frame plan runs the clip's effects as GPU ops on the source frame.
    fn gpu_fx(&self, t: Tick) -> bool {
        match plan::plan_frame(&self.p, self.seq, t, RenderOptions::default(), &self.map) {
            plan::FramePlan::Layers { layers, .. } => layers.iter().any(|l| l.fx.as_ref().is_some_and(|f| !f.ops.is_empty())),
            _ => false,
        }
    }
}

fn worst(a: &Image, b: &Image) -> f32 {
    assert_eq!((a.w, a.h), (b.w, b.h));
    a.px.iter().zip(&b.px).map(|(x, y)| (x - y).abs()).fold(0.0, f32::max)
}

/// "Visibly another picture".
const VISIBLE: f32 = 0.03;

fn t(frame: i64) -> Tick {
    RATE.tick_of(frame)
}

fn opts() -> RenderOptions {
    RenderOptions::default()
}

#[test]
fn a_source_effect_renders_as_the_same_effect_on_the_clip() {
    let chains: Vec<(&str, Vec<EffectInstance>)> = vec![
        ("lumetri with a library input LUT", vec![lumetri_lut(SWAP)]),
        ("lumetri with a built-in input LUT", vec![lumetri_lut("builtin:slog3-sgamut3cine-to-rec709")]),
        ("lumetri with basic and creative settings", vec![lumetri_grade()]),
        ("a blur", vec![blur()]),
        ("a grade, a LUT and a blur", vec![lumetri_grade(), lumetri_lut(WARM), blur()]),
        (
            "a disabled effect",
            vec![{
                let mut e = lumetri_grade();
                e.enabled = false;
                e
            }],
        ),
    ];
    for generator in [Generator::Demo(DemoScene::OceanSunset), Generator::ColorMatte { color: [0.8, 0.4, 0.1, 0.5] }] {
        for (name, chain) in &chains {
            // Motion and Opacity come after the chain, whichever list it is in
            let moved = |r: Rig| {
                let (seq, clip) = (r.seq, r.clip);
                let mut r = r;
                let c = r.clip_mut(seq, clip);
                c.effect_mut("motion").unwrap().params.get_mut("scale").unwrap().value = ParamValue::Float(60.0);
                c.effect_mut("motion").unwrap().params.get_mut("rotation").unwrap().value = ParamValue::Float(10.0);
                c.effect_mut("opacity").unwrap().params.get_mut("opacity").unwrap().value = ParamValue::Float(80.0);
                r
            };
            let on_media = moved(Rig::new(generator.clone()).source(chain.clone()));
            let on_clip = moved(Rig::new(generator.clone()).on_clip(chain.clone()));
            let plain = moved(Rig::new(generator.clone()));
            for scale in [1.0, 0.5] {
                let o = RenderOptions { scale, ..opts() };
                let (a, b) = (on_media.render(t(5), o), on_clip.render(t(5), o));
                assert_eq!(worst(&a, &b), 0.0, "{name} on {generator:?} at {scale}: the CPU renders differ");
                let (pa, pb) = (on_media.planned(t(5), o), on_clip.planned(t(5), o));
                assert_eq!(worst(&pa, &pb), 0.0, "{name} on {generator:?} at {scale}: the GPU plans differ");
                if *name != "a disabled effect" && matches!(generator, Generator::Demo(_)) {
                    assert!(worst(&a, &plain.render(t(5), o)) > VISIBLE, "{name}: the effect changes the picture");
                }
            }
        }
    }
}

#[test]
fn a_source_chain_reaches_the_gpu_exactly_when_the_clips_would() {
    // basic / creative Lumetri and a blur are GPU ops: the plan runs them on the source frame
    for chain in [vec![lumetri_grade()], vec![blur()], vec![lumetri_grade(), blur()]] {
        let (media, clip) = (Rig::ocean().source(chain.clone()), Rig::ocean().on_clip(chain));
        assert!(clip.gpu_fx(t(5)), "the same effects on the clip are GPU ops");
        assert!(media.gpu_fx(t(5)), "so are the media's");
        // and the GPU ops give the CPU reference's picture
        assert!(
            worst(&media.planned(t(5), opts()), &media.render(t(5), opts())) < 2e-3,
            "{}",
            worst(&media.planned(t(5), opts()), &media.render(t(5), opts()))
        );
    }
    // a Lumetri input LUT has no GPU op: both go to the CPU, the picture drawn is the CPU's
    let (media, clip) = (Rig::ocean().source(vec![lumetri_lut(SWAP)]), Rig::ocean().on_clip(vec![lumetri_lut(SWAP)]));
    assert!(!media.gpu_fx(t(5)) && !clip.gpu_fx(t(5)));
    assert_eq!(worst(&media.planned(t(5), opts()), &media.render(t(5), opts())), 0.0);
    // a disabled effect leaves the plain GPU path; one the GPU lacks (a source chain with a CPU-only
    // effect) takes the CPU
    let mut off = lumetri_grade();
    off.enabled = false;
    assert!(!Rig::ocean().source(vec![off]).gpu_fx(t(5)));
    let cpu_only = Rig::ocean().source(vec![lumetri_grade(), fx("find_edges")]);
    assert!(!cpu_only.gpu_fx(t(5)));
    assert!(worst(&cpu_only.planned(t(5), opts()), &cpu_only.render(t(5), opts())) < 1e-6);
    assert!(worst(&cpu_only.render(t(5), opts()), &Rig::ocean().render(t(5), opts())) > VISIBLE);
}

#[test]
fn the_source_chain_runs_before_the_clips_own_effects() {
    // source A, clip B  ==  clip [A, B]  !=  clip [B, A]
    let both = Rig::ocean().source(vec![lumetri_lut(SWAP)]).on_clip(vec![lumetri_lut(WARM)]);
    let chain = Rig::ocean().on_clip(vec![lumetri_lut(SWAP), lumetri_lut(WARM)]);
    let reversed = Rig::ocean().on_clip(vec![lumetri_lut(WARM), lumetri_lut(SWAP)]);
    for planned in [false, true] {
        let img = |r: &Rig| if planned { r.planned(t(5), opts()) } else { r.render(t(5), opts()) };
        assert_eq!(worst(&img(&both), &img(&chain)), 0.0, "source first (planned: {planned})");
        assert!(worst(&img(&both), &img(&reversed)) > VISIBLE, "the order matters (planned: {planned})");
    }
    // order inside the source list is the chain order too
    let two = Rig::ocean().source(vec![lumetri_lut(SWAP), lumetri_lut(WARM)]);
    assert_eq!(worst(&two.render(t(5), opts()), &chain.render(t(5), opts())), 0.0);
    // with effects off (Toggle Effects) neither list runs
    let off = RenderOptions { effects: false, ..opts() };
    let plain = Rig::ocean();
    assert_eq!(worst(&both.render(t(5), off), &plain.render(t(5), off)), 0.0);
    assert_eq!(worst(&both.planned(t(5), off), &plain.planned(t(5), off)), 0.0);
}

#[test]
fn a_clips_temporal_effect_sees_the_source_effects_on_the_other_frames() {
    // Echo mixes earlier frames of the clip: they carry the source chain as the clip's own would
    let mut echo = fx("echo");
    echo.params.get_mut("time").unwrap().value = ParamValue::Float(-0.1);
    echo.params.get_mut("count").unwrap().value = ParamValue::Float(2.0);
    let source = Rig::ocean().source(vec![lumetri_lut(SWAP)]).on_clip(vec![echo.clone()]);
    let clip = Rig::ocean().on_clip(vec![lumetri_lut(SWAP), echo.clone()]);
    let plain = Rig::ocean().on_clip(vec![echo]);
    let (a, b) = (source.render(t(12), opts()), clip.render(t(12), opts()));
    assert_eq!(worst(&a, &b), 0.0);
    assert!(worst(&a, &plain.render(t(12), opts())) > VISIBLE);
}

#[test]
fn effects_a_source_cannot_have_are_refused_and_skipped() {
    for ok in ["lumetri", "gaussian_blur", "invert", "black_white", "find_edges"] {
        assert_eq!(effects::source_effect_refusal(ok), None, "{ok}");
    }
    for no in [
        "motion",
        "opacity",
        "time_remap",
        "volume",
        "cross_dissolve",
        "echo",
        "posterize_time",
        "warp_stabilizer",
        "auto_reframe",
        "track_matte",
        "compound_blur",
        "timecode",
        "clip_name",
        "grow",
        "move",
        "spin",
        "no_such_effect",
        filmcraft_project::graphic::TEXT_LAYER,
    ] {
        assert!(effects::source_effect_refusal(no).is_some(), "{no}");
    }
    // a damaged or hand-edited project holding them anyway renders as without them
    let base = Rig::ocean().render(t(5), opts());
    for id in ["motion", "opacity", "echo", "posterize_time", "volume", "cross_dissolve", "track_matte"] {
        let mut e = fx(id);
        if let Some(p) = e.params.get_mut("scale") {
            p.value = ParamValue::Float(10.0);
        }
        let r = Rig::ocean().source(vec![e]);
        assert_eq!(worst(&r.render(t(5), opts()), &base), 0.0, "{id}");
        assert_eq!(worst(&r.planned(t(5), opts()), &base), 0.0, "{id} (plan)");
    }
}

#[test]
fn subclips_nests_and_the_source_monitor_show_the_source_effects() {
    let want = Rig::ocean().on_clip(vec![lumetri_lut(SWAP), blur()]).render(t(5), opts());
    let base = Rig::ocean().render(t(5), opts());
    let mut r = Rig::ocean().source(vec![lumetri_lut(SWAP), blur()]);
    assert!(worst(&r.render(t(5), opts()), &want) == 0.0 && worst(&want, &base) > VISIBLE);
    // a subclip of the media shares its list
    let sub = r.p.add_item(
        "sub",
        Label::Iris,
        ItemKind::Subclip { parent: r.media, range: TimeRange::new(Tick::ZERO, Tick(5 * TICKS_PER_SECOND)), restrict_trims: false },
        None,
    );
    let source = r.map.0[&r.media].clone();
    r.map.0.insert(sub, source);
    assert_eq!(r.p.source_effects_of(sub).len(), 2);
    assert_eq!(r.p.source_effects_of(r.seq).len(), 0, "a sequence has none");
    let seq2 = r.p.new_sequence("sub seq", SequenceSettings { width: 320, height: 180, frame_rate: RATE, ..Default::default() }, 1, 1, None);
    r.put(seq2, sub);
    let img = render_sequence(&r.p, seq2, t(5), opts(), &r.map);
    assert_eq!(worst(&img, &want), 0.0);
    assert_eq!(worst(&plan::execute_cpu(&plan::plan_frame(&r.p, seq2, t(5), opts(), &r.map)), &want), 0.0);
    // a nested sequence: its clip carries the media's effects, the nest is drawn like any other
    let outer = r.p.new_sequence("outer", SequenceSettings { width: 320, height: 180, frame_rate: RATE, ..Default::default() }, 1, 1, None);
    r.put(outer, r.seq);
    let nested = render_sequence(&r.p, outer, t(5), opts(), &r.map);
    assert_eq!(worst(&nested, &want), 0.0);
    assert_eq!(worst(&plan::execute_cpu(&plan::plan_frame(&r.p, outer, t(5), opts(), &r.map)), &want), 0.0);
    // the Source Monitor and bin thumbnails (`render_item`), at the shown frame's media time, for
    // the media and for its subclip; a 320x180 sequence draws the same picture 1:1
    for item in [r.media, sub] {
        let monitor = render_item(&r.p, item, t(5), 1.0, &r.map).unwrap();
        assert!(worst(&monitor, &want) < 1e-5, "{}", worst(&monitor, &want));
        let thumb = render_item(&r.p, item, t(5), 0.25, &r.map).unwrap();
        assert_eq!((thumb.w, thumb.h), (80, 45));
        let plain = Rig::ocean();
        let plain_thumb = render_item(&plain.p, plain.media, t(5), 0.25, &plain.map).unwrap();
        assert!(worst(&thumb, &plain_thumb) > VISIBLE, "the thumbnail shows the effects");
    }
    // no effects, no change: the monitor shows what it always did
    let plain = Rig::ocean();
    let a = render_item(&plain.p, plain.media, t(5), 1.0, &plain.map).unwrap();
    assert_eq!(worst(&a, &base), 0.0);
}

#[test]
fn source_effect_keyframes_are_in_media_time() {
    // the clip plays the media from 2 s (a trimmed clip); the exposure keyframes sit at media
    // times 2 s (0 stops) and 4 s (+2 stops)
    let lumetri_at = |keyframed: bool, stops: f64| {
        let mut e = find_effect("lumetri").unwrap().instance();
        let exposure = e.params.get_mut("exposure").unwrap();
        if keyframed {
            exposure.put_keyframe(Tick(2 * TICKS_PER_SECOND), ParamValue::Float(0.0));
            exposure.put_keyframe(Tick(4 * TICKS_PER_SECOND), ParamValue::Float(2.0));
        } else {
            exposure.value = ParamValue::Float(stops);
        }
        e
    };
    let trimmed = |mut r: Rig| {
        let (seq, clip) = (r.seq, r.clip);
        let c = r.clip_mut(seq, clip);
        c.source_in = Tick(2 * TICKS_PER_SECOND);
        r
    };
    let source = trimmed(Rig::ocean().source(vec![lumetri_at(true, 0.0)]));
    let clip = trimmed(Rig::ocean().on_clip(vec![lumetri_at(true, 0.0)]));
    // timeline frame 24 is media time 3 s: halfway, +1 stop
    let fixed = trimmed(Rig::ocean().on_clip(vec![lumetri_at(false, 1.0)]));
    let (a, b, c) = (source.render(t(24), opts()), clip.render(t(24), opts()), fixed.render(t(24), opts()));
    assert_eq!(worst(&a, &b), 0.0);
    assert!(worst(&a, &c) < 1e-4, "{}", worst(&a, &c));
    // frame 0 is media time 2 s: the first keyframe, 0 stops: the plain picture
    let plain = trimmed(Rig::ocean());
    assert!(worst(&source.render(t(0), opts()), &plain.render(t(0), opts())) < 1e-4);
    // and the end of the clip, media time 4 s: +2 stops
    let end = trimmed(Rig::ocean().on_clip(vec![lumetri_at(false, 2.0)]));
    assert!(worst(&source.render(t(47), opts()), &end.render(t(47), opts())) < 0.05);
    // a clip played at half speed reads media time too: timeline frame 48 is media time 3 s
    let slow = |mut r: Rig| {
        let (seq, clip) = (r.seq, r.clip);
        let c = r.clip_mut(seq, clip);
        c.speed = 0.5;
        c.duration = Tick(c.duration.0 * 2);
        r
    };
    let (sa, sb) = (slow(trimmed(Rig::ocean().source(vec![lumetri_at(true, 0.0)]))), slow(trimmed(Rig::ocean().on_clip(vec![lumetri_at(false, 1.0)]))));
    assert!(worst(&sa.render(t(48), opts()), &sb.render(t(48), opts())) < 1e-4);
}

#[test]
fn source_effects_change_the_preview_hash_and_the_cost() {
    use crate::preview::{Need, Segment, segment_at, video_segments};
    let seg = |r: &Rig| -> Segment { segment_at(&video_segments(&r.p, r.seq), 5).unwrap().clone() };
    let plain = Rig::ocean();
    let base = seg(&plain);
    assert_eq!(base.need, Need::None, "plain media plays natively");
    // adding one invalidates cached previews and ends native playback
    let with = Rig::ocean().source(vec![lumetri_grade()]);
    let added = seg(&with);
    assert_ne!(added.hash, base.hash);
    assert_eq!(added.need, Need::Realtime);
    assert!(added.cost_ms > base.cost_ms, "{} vs {}", added.cost_ms, base.cost_ms);
    // changing a parameter
    let mut g = lumetri_grade();
    g.params.get_mut("exposure").unwrap().value = ParamValue::Float(0.9);
    let changed = seg(&Rig::ocean().source(vec![g]));
    assert_ne!(changed.hash, added.hash);
    // keyframing one
    let mut k = lumetri_grade();
    k.params.get_mut("exposure").unwrap().put_keyframe(Tick(TICKS_PER_SECOND), ParamValue::Float(1.0));
    assert_ne!(seg(&Rig::ocean().source(vec![k])).hash, added.hash);
    // toggling it off: a different picture (the same as none), a different hash from both
    let mut off = lumetri_grade();
    off.enabled = false;
    let toggled = seg(&Rig::ocean().source(vec![off]));
    assert_ne!(toggled.hash, added.hash);
    assert_ne!(toggled.hash, base.hash, "the list is part of the media");
    // the same list is the same hash, wherever the media is used
    assert_eq!(seg(&Rig::ocean().source(vec![lumetri_grade()])).hash, added.hash);
    // a library LUT a source effect names: removing it (it then renders unchanged) changes the hash
    let lut = Rig::ocean().source(vec![lumetri_lut(SWAP)]);
    let with_lut = seg(&lut);
    let mut gone = Rig::ocean().source(vec![lumetri_lut(SWAP)]);
    gone.p.luts.retain(|l| l.id != "swap");
    assert_ne!(seg(&gone).hash, with_lut.hash);
    // a subclip's segment hashes the media's list
    let mut r = Rig::ocean().source(vec![lumetri_grade()]);
    let sub = r.p.add_item(
        "sub",
        Label::Iris,
        ItemKind::Subclip { parent: r.media, range: TimeRange::new(Tick::ZERO, Tick(5 * TICKS_PER_SECOND)), restrict_trims: false },
        None,
    );
    let s2 = r.p.new_sequence("s2", SequenceSettings { width: 320, height: 180, frame_rate: RATE, ..Default::default() }, 1, 1, None);
    r.put(s2, sub);
    let before = segment_at(&video_segments(&r.p, s2), 5).unwrap().hash.clone();
    r.media_mut().source_effects.clear();
    assert_ne!(segment_at(&video_segments(&r.p, s2), 5).unwrap().hash, before);
}

#[test]
fn a_project_without_source_effects_renders_and_hashes_as_before() {
    // the empty list is not in the file, so existing previews keep their hashes
    let r = Rig::ocean();
    let json = serde_json::to_string(&r.p.item(r.media).unwrap().kind).unwrap();
    assert!(!json.contains("source_effects"));
    assert!(r.p.source_effects_of(r.media).is_empty());
    assert!(!has_clip_effects(&r.p, &r.p.sequence(r.seq).unwrap().video_tracks[0].items[0]));
    let with = Rig::ocean().source(vec![blur()]);
    assert!(has_clip_effects(&with.p, &with.p.sequence(with.seq).unwrap().video_tracks[0].items[0]));
}
