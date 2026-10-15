//! What the Lumetri Color and Effect Controls panels edit: the source clip effects of the media
//! open in the Source Monitor ("Source · media"), or the effects of the selected clip in the
//! active sequence ("sequence · clip"), as in Premiere.
//!
//! Each panel opens with a pair of pills that pick one. The pick is shared: choosing in one panel
//! chooses in the other. It follows the editor's context: opening media in the Source Monitor, or
//! focusing the Source Monitor, picks Source; selecting a clip or a transition in the Timeline
//! picks Sequence; a click on a pill holds until one of those happens again. A pill with nothing
//! behind it (no media open, no clip selected) is disabled, and the other one is shown instead.
//!
//! An effect dragged from the Effects panel onto Effect Controls (showing Source) or onto the
//! Source Monitor becomes a source clip effect of the open media ([`accept_effect_drop`]).
//!
//! Automation ids: `<panel>.target.source` and `<panel>.target.sequence`, where `<panel>` is
//! `lumetri` or `effectControls`. The pick is `ui.fx_target.pick` in `ui.inspect`, and the control
//! channel sets it with `ui.set {"fxTarget": "source" | "sequence"}`.

use egui::{Rect, Sense, Stroke, StrokeKind, pos2, vec2};
use filmcraft_project::{ClipId, ItemId};
use filmcraft_time::Tick;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::FilmcraftApp;
use crate::dock::PanelKind;
use crate::icons::{self, Icon};
use crate::theme::Tokens;

/// Which of the two the panels show when both are possible.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Pick {
    Source,
    #[default]
    Sequence,
}

impl Pick {
    pub fn parse(s: &str) -> Option<Pick> {
        match s.to_ascii_lowercase().as_str() {
            "source" => Some(Pick::Source),
            "sequence" => Some(Pick::Sequence),
            _ => None,
        }
    }
}

/// The pick, the Lumetri Color of the target the Lumetri panel edits, and what the pick last
/// reacted to.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct TargetState {
    pub pick: Pick,
    /// Which Lumetri Color of the target the Lumetri panel edits (0 = the first), for `instance_of`.
    pub instance: usize,
    #[serde(skip)]
    instance_of: Option<Fx>,
    #[serde(skip)]
    seen: Option<Seen>,
}

/// The context [`sync`] saw last frame.
#[derive(Clone, Debug, PartialEq)]
struct Seen {
    source: Option<ItemId>,
    selection: (Vec<ClipId>, Vec<filmcraft_project::TransitionId>),
    source_focused: bool,
}

/// What an effect command lands on: `{"clip": id}` (a timeline clip) or `{"item": id}` (the source
/// clip effects of a media item). Ids are one space, so an `Fx` also names its widgets.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Fx {
    pub id: u64,
    pub source: bool,
}

impl Fx {
    pub fn clip(c: ClipId) -> Fx {
        Fx { id: c.0, source: false }
    }
    pub fn item(i: ItemId) -> Fx {
        Fx { id: i.0, source: true }
    }
    /// The command parameter that names this target.
    pub fn key(self) -> &'static str {
        if self.source { "item" } else { "clip" }
    }
    /// `params` (an object) with the target added.
    pub fn with(self, mut params: Value) -> Value {
        params[self.key()] = json!(self.id);
        params
    }
    /// The timeline clip, when this is one.
    pub fn as_clip(self) -> Option<ClipId> {
        (!self.source).then_some(ClipId(self.id))
    }
    /// The command that moves the playhead the effects' keyframes sit on to `t`: the Timeline's
    /// (a timeline time) for a clip, the Source Monitor's (a media time) for source clip effects.
    pub fn seek(self, t: Tick) -> (String, Value) {
        (if self.source { "source.setPlayhead" } else { "playhead.set" }.to_string(), json!({"time": t.0}))
    }
}

/// The media open in the Source Monitor, when it has a picture to put effects on.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SourceRef {
    /// The item open there (a media item or a subclip of one).
    pub item: ItemId,
    /// The media the effects belong to.
    pub media: ItemId,
    pub name: String,
}

pub fn source_ref(app: &FilmcraftApp) -> Option<SourceRef> {
    let item = app.session.state.source_item?;
    let name = app.session.project.item(item)?.name.clone();
    let (media, clip, _) = app.session.project.resolve_media(item)?;
    clip.info.video.as_ref()?;
    Some(SourceRef { item, media, name })
}

/// What the panels show this frame.
#[derive(Clone, Debug)]
pub struct Choice {
    pub source: Option<SourceRef>,
    /// The pick that has something behind it: None when neither has.
    pub active: Option<Pick>,
}

/// The shown pick, given whether the panel has a subject in the sequence (a clip, or a transition).
pub fn choose(app: &FilmcraftApp, has_sequence_subject: bool) -> Choice {
    let source = source_ref(app);
    // (the pick, when it has something behind it; else whichever has)
    let active = match (app.ui.fx_target.pick, source.is_some(), has_sequence_subject) {
        (Pick::Source, true, _) | (_, true, false) => Some(Pick::Source),
        (_, _, true) => Some(Pick::Sequence),
        _ => None,
    };
    Choice { source, active }
}

/// Follow the editor's context (call once a frame, before the panels): see the module docs.
pub fn sync(app: &mut FilmcraftApp) {
    let now = Seen {
        source: source_ref(app).map(|s| s.item),
        selection: (app.session.state.selection.clone(), app.session.state.transition_selection.clone()),
        source_focused: app.ui.focused == PanelKind::Source,
    };
    if let Some(before) = &app.ui.fx_target.seen {
        let selected = !now.selection.0.is_empty() || !now.selection.1.is_empty();
        if selected && now.selection != before.selection {
            app.ui.fx_target.pick = Pick::Sequence;
        }
        // (the Source Monitor is the latest context when both changed in one frame)
        if now.source.is_some() && (now.source != before.source || now.source_focused && !before.source_focused) {
            app.ui.fx_target.pick = Pick::Source;
        }
    }
    app.ui.fx_target.seen = Some(now);
}

/// The Lumetri Color of `fx` the Lumetri panel edits, an index among the target's Lumetri Colors;
/// reset to the first when the target changes, kept inside the list when an effect goes.
pub fn instance(app: &mut FilmcraftApp, fx: Fx, count: usize) -> usize {
    let st = &mut app.ui.fx_target;
    if st.instance_of != Some(fx) {
        st.instance_of = Some(fx);
        st.instance = 0;
    }
    st.instance = st.instance.min(count.saturating_sub(1));
    st.instance
}

/// Choose Lumetri Color number `n` of the target.
pub fn set_instance(app: &mut FilmcraftApp, n: usize) {
    app.ui.fx_target.instance = n;
}

/// An effect dragged from the Effects panel and let go over `rect` (a panel showing the media open
/// in the Source Monitor) becomes a source clip effect of that media: a Lumetri preset applies as
/// its Lumetri Color. The engine refuses what cannot be one (a transition, an audio effect…) and
/// its reason goes to the status bar. While the effect is over `rect` the panel gets an outline.
/// Returns whether the effect was dropped here.
pub fn accept_effect_drop(app: &mut FilmcraftApp, ui: &egui::Ui, rect: Rect) -> bool {
    let Some(src) = source_ref(app) else { return false };
    let Some(effect) = crate::panels::dragged_effect(ui) else { return false };
    let ctx = ui.ctx();
    if !ctx.pointer_hover_pos().is_some_and(|p| rect.contains(p)) {
        return false;
    }
    ui.painter().rect_stroke(rect.shrink(1.0), 0.0, Stroke::new(2.0, app.tokens.accent), StrokeKind::Inside);
    if !ctx.input(|i| i.pointer.any_released()) {
        return false;
    }
    let outcome = if effect.starts_with("preset:") {
        // (an effect preset sets a clip up, Motion and all)
        Err(tl!("Effect presets apply to clips in the Timeline, not to source clips").to_string())
    } else if let Some(name) = effect.strip_prefix(crate::panels::LUMETRI_PRESET_PREFIX) {
        app.session.execute("lumetri.applyPreset", json!({"name": name, "item": src.media.0})).map_err(|e| e.to_string())
    } else {
        app.session.execute("effects.apply", json!({"effect": effect, "item": src.media.0})).map_err(|e| e.to_string())
    };
    match outcome {
        Ok(_) => app.ui.fx_target.pick = Pick::Source,
        Err(e) => app.ui.status = e,
    }
    crate::panels::clear_drag(ui);
    true
}

const PILL_H: f32 = 24.0;

/// Draw the pair of pills in `rect` (one row), take clicks on them, and say what is shown now.
/// `prefix` names the automation ids; `sequence_subject` is the name of the clip (or transition)
/// the panel would show for the sequence, if any.
pub fn pills(app: &mut FilmcraftApp, ui: &mut egui::Ui, rect: Rect, prefix: &str, sequence_subject: Option<&str>) -> Choice {
    let sequence_name = app.session.state.active_sequence.and_then(|s| app.session.project.item(s)).map(|i| i.name.clone());
    let source = source_ref(app);
    let cell = (rect.width() - 6.0) / 2.0;
    let row = Rect::from_min_size(rect.min, vec2(rect.width(), PILL_H));
    let source_rect = Rect::from_min_size(row.min, vec2(cell, PILL_H));
    let sequence_rect = Rect::from_min_size(row.min + vec2(cell + 6.0, 0.0), vec2(cell, PILL_H));
    let before = choose(app, sequence_subject.is_some());
    let source_text = match &source {
        Some(s) => tlf!("Source · {name}", name = s.name),
        None => tl!("Source").to_string(),
    };
    let sequence_text = match (&sequence_name, sequence_subject) {
        (Some(q), Some(c)) => format!("{q} · {c}"),
        (Some(q), None) => q.clone(),
        (None, Some(c)) => c.to_string(),
        (None, None) => tl!("Sequence").to_string(),
    };
    let source_tip = if source.is_some() { source_text.clone() } else { tl!("Open media in the Source monitor to edit its source clip effects").to_string() };
    let sequence_tip = if sequence_subject.is_some() { sequence_text.clone() } else { tl!("Select a clip in the Timeline to edit its effects").to_string() };
    let mut pick = None;
    let source_resp = pill(
        app,
        ui,
        source_rect,
        &format!("{prefix}.target.source"),
        &source_text,
        &source_tip,
        Icon::SourceClip,
        before.active == Some(Pick::Source),
        source.is_some(),
    );
    let sequence_resp = pill(
        app,
        ui,
        sequence_rect,
        &format!("{prefix}.target.sequence"),
        &sequence_text,
        &sequence_tip,
        Icon::Sequence,
        before.active == Some(Pick::Sequence),
        sequence_subject.is_some(),
    );
    if source_resp.clicked() {
        pick = Some(Pick::Source);
    }
    if sequence_resp.clicked() {
        pick = Some(Pick::Sequence);
    }
    if let Some(p) = pick {
        app.ui.fx_target.pick = p;
        return choose(app, sequence_subject.is_some());
    }
    before
}

/// One pill: filled when it is the shown one, outlined when not, dimmed and inert when it has
/// nothing behind it. The text is cut off with "…" and the tooltip gives all of it.
#[allow(clippy::too_many_arguments)]
fn pill(app: &mut FilmcraftApp, ui: &mut egui::Ui, r: Rect, id: &str, text: &str, tip: &str, icon: Icon, active: bool, enabled: bool) -> egui::Response {
    let t = app.tokens;
    let resp = ui.interact(r, egui::Id::new(("fx-target", id)), if enabled { Sense::click() } else { Sense::hover() });
    let hovered = enabled && resp.hovered();
    let fill = if active {
        t.pill_active_bg
    } else if hovered {
        t.hover
    } else {
        t.panel_bg
    };
    ui.painter().rect_filled(r, 4.0, fill);
    if !active {
        ui.painter().rect_stroke(r, 4.0, Stroke::new(1.0, t.separator), StrokeKind::Inside);
    }
    let ink = if !enabled {
        t.text_faint
    } else if active {
        t.text
    } else {
        t.text_dim
    };
    icons::paint(ui.painter(), egui::Rect::from_center_size(pos2(r.min.x + 14.0, r.center().y), vec2(14.0, 14.0)), icon, ink);
    crate::panels::effect_controls::row_label(ui, pos2(r.min.x + 26.0, r.center().y), text, r.width() - 26.0 - 8.0, Tokens::ui(11.5), ink);
    app.auto.add_response(id, &resp, text);
    // (a pill with nothing behind it senses only hovering: say it is disabled)
    if let Some(element) = app.auto.elements.last_mut() {
        element.enabled = Some(enabled);
    }
    resp.on_hover_text(tip)
}
