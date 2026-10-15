//! Source clip effects (Premiere's master clip effects): effects stored on a media item
//! (`MediaClip::source_effects`) that render first, ahead of every timeline clip's own effects,
//! wherever the footage is used (see `filmcraft_render::clip_effects`). Subclips share their
//! media's list.
//!
//! The effect commands (`effects.*`, `lumetri.*`) address a timeline clip by `clip` or a media
//! item by `item`, a project item id that is media or a subclip of media (resolved to the media).
//! Passing both is an error. Both targets run the same code: [`edit_effects`] hands the command a
//! timeline-clip-shaped view of the media (the whole picture, source time 0 at media time 0, normal
//! speed) holding the source effects, so keyframes are in media time and one undo step covers the
//! edit.
//! `effects.list {"item": id}` reads the list.

use filmcraft_project::{ClipId, EffectDef, ItemId, ItemKind, Project, TrackItem};
use filmcraft_time::Tick;
use serde_json::{Value, json};

use crate::commands::{bad, clip_p};
use crate::{EditorState, Result, Session};

/// Where an effect edit lands.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum EffectTarget {
    /// A clip of the active sequence.
    Clip(ClipId),
    /// A media item: its source clip effects.
    Media(ItemId),
}

impl EffectTarget {
    pub(crate) fn media(self) -> Option<ItemId> {
        match self {
            EffectTarget::Media(m) => Some(m),
            EffectTarget::Clip(_) => None,
        }
    }

    /// A name for undo-merge keys that tells the two kinds of target apart.
    pub(crate) fn key(self) -> String {
        match self {
            EffectTarget::Clip(c) => c.0.to_string(),
            EffectTarget::Media(m) => format!("item{}", m.0),
        }
    }
}

/// An id a caller gave: a non-negative whole number (`7`, or `7.0` from a client that writes floats).
fn id_of(v: &Value) -> Option<u64> {
    v.as_u64().or_else(|| v.as_f64().filter(|f| f.fract() == 0.0 && (0.0..9.0e15).contains(f)).map(|f| f as u64))
}

pub(crate) fn named(p: &Value, k: &str) -> Option<Value> {
    p.get(k).filter(|v| !v.is_null()).cloned()
}

/// Whether a command's params name a media item (`item` / `items`) instead of timeline clips.
pub(crate) fn names_item(p: &Value) -> bool {
    named(p, "item").is_some() || named(p, "items").is_some()
}

/// `clip`/`clips` together with `item`/`items` is a mistake, not a choice.
fn exclusive(p: &Value, cmd: &str) -> Result<()> {
    let item = names_item(p);
    let clip = named(p, "clip").is_some() || named(p, "clips").is_some();
    if item && clip {
        return Err(bad(cmd, "pass `clip` (a timeline clip) or `item` (a project item's source clip effects), not both"));
    }
    Ok(())
}

/// The media (with a picture) that project item `v` stands for: a media item, or a subclip of one.
fn media_of(s: &Session, v: &Value, cmd: &str) -> Result<ItemId> {
    let id = id_of(v).ok_or_else(|| bad(cmd, format!("`item` must be a project item id (a whole number), not {v}")))?;
    let (media, clip, _) = s.project.resolve_media(ItemId(id)).ok_or_else(|| {
        bad(cmd, format!("item {id} is not media or a subclip of media (sequences, graphics and adjustment layers have no source clip effects)"))
    })?;
    if clip.info.video.is_none() {
        return Err(bad(cmd, format!("item {id} has no picture: source clip effects are video effects")));
    }
    Ok(media)
}

/// The target of a command that takes one: `clip`, or `item` for the source clip effects of a
/// media item or subclip.
pub(crate) fn target_p(s: &Session, p: &Value, cmd: &str) -> Result<EffectTarget> {
    exclusive(p, cmd)?;
    if let Some(v) = named(p, "item") {
        return Ok(EffectTarget::Media(media_of(s, &v, cmd)?));
    }
    if named(p, "items").is_some() {
        return Err(bad(cmd, "this command takes one `item`, not `items`"));
    }
    clip_p(p, "clip").map(EffectTarget::Clip).ok_or_else(|| bad(cmd, "need `clip` (a timeline clip) or `item` (a project item)"))
}

/// The media items `item` / `items` name, for a command that applies to several (None when the
/// command names neither, so it falls back to timeline clips).
pub(crate) fn items_p(s: &Session, p: &Value, cmd: &str) -> Result<Option<Vec<ItemId>>> {
    exclusive(p, cmd)?;
    let (one, many) = (named(p, "item"), named(p, "items"));
    if one.is_some() && many.is_some() {
        return Err(bad(cmd, "pass `item` or `items`, not both"));
    }
    let ids = match (one, many) {
        (Some(v), None) => vec![v],
        (None, Some(Value::Array(a))) if !a.is_empty() => a,
        (None, Some(Value::Array(_))) => return Err(bad(cmd, "`items` is empty")),
        (None, Some(v)) => return Err(bad(cmd, format!("`items` must be a list of project item ids, not {v}"))),
        _ => return Ok(None),
    };
    let mut out: Vec<ItemId> = Vec::new();
    for v in &ids {
        let m = media_of(s, v, cmd)?;
        if !out.contains(&m) {
            out.push(m);
        }
    }
    Ok(Some(out))
}

/// The media time a command with no explicit `time` / `seconds` works at: the Source Monitor's
/// playhead when this media is open there, else its start.
pub(crate) fn media_time_p(s: &Session, p: &Value, media: ItemId) -> Tick {
    if let Some(t) = p.get("time").and_then(Value::as_i64) {
        return Tick(t);
    }
    if let Some(sec) = p.get("seconds").and_then(Value::as_f64).filter(|v| v.is_finite()) {
        return Tick::from_seconds_f64(sec);
    }
    let open = s.state.source_item.and_then(|i| s.project.resolve_media(i)).map(|(m, ..)| m);
    if open == Some(media) { s.state.source_playhead } else { Tick::ZERO }
}

/// The picture size of a media item (full resolution), for resolving "auto" points.
pub(crate) fn media_frame(p: &Project, media: ItemId) -> (u32, u32) {
    p.source_size(media).unwrap_or((1920, 1080))
}

/// Run `f` on the effects of `target` as one undoable edit named `label` (consecutive edits with
/// the same `merge` key share a step). For a media item `f` sees a timeline-clip-shaped view of
/// it (`start` 0, `source_in` 0, speed 1, the media's duration) whose `effects` are the source
/// clip effects; whatever `f` leaves there becomes the media's list.
pub(crate) fn edit_effects<R>(
    s: &mut Session,
    target: EffectTarget,
    label: &str,
    merge: Option<&str>,
    f: impl FnOnce(&mut TrackItem) -> Result<R>,
) -> Result<R> {
    match target {
        EffectTarget::Clip(c) => s.edit_sequence_as(label, merge, |q, _, _| {
            let (_, it) = q.find_item_mut(c).ok_or(filmcraft_edit::EditError::NoItem(c))?;
            f(it)
        }),
        EffectTarget::Media(m) => {
            let body = move |pr: &mut Project, _: &mut EditorState| {
                let mut view = pr.media_view(m).ok_or_else(|| bad("source effects", "the item is not media"))?;
                let Some(ItemKind::Media(c)) = pr.item_mut(m).map(|i| &mut i.kind) else {
                    return Err(bad("source effects", "the item is not media"));
                };
                view.effects = std::mem::take(&mut c.source_effects);
                let r = f(&mut view);
                c.source_effects = view.effects;
                r
            };
            match merge {
                Some(key) => s.edit_merged(label, key, body),
                None => s.edit(label, body),
            }
        }
    }
}

/// `effects.apply` on media: add `def` as a source clip effect of each of `items` (one undo step).
pub(crate) fn apply(s: &mut Session, items: &[ItemId], def: &'static EffectDef) -> Result<Value> {
    const CMD: &str = "effects.apply";
    if let Some(why) = filmcraft_render::effects::source_effect_refusal(def.id) {
        return Err(bad(CMD, format!("{} cannot be a source clip effect: {why}", def.name)));
    }
    s.edit(&format!("Apply {}", def.name), |pr, _| {
        for m in items {
            let size = media_frame(pr, *m);
            let mut inst = def.instance();
            filmcraft_project::resolve_auto_points(&mut inst, size, size);
            let Some(ItemKind::Media(c)) = pr.item_mut(*m).map(|i| &mut i.kind) else {
                return Err(bad(CMD, "the item is not media"));
            };
            c.source_effects.push(inst);
        }
        Ok(())
    })?;
    Ok(json!({"items": items.iter().map(|i| i.0).collect::<Vec<_>>(), "effect": def.id}))
}

/// `effects.list {"item": id}`: the source clip effects of a media item or subclip, in render order.
pub(crate) fn list(s: &Session, p: &Value) -> Result<Value> {
    const CMD: &str = "effects.list";
    let v = named(p, "item").ok_or_else(|| bad(CMD, "need `item`"))?;
    let media = media_of(s, &v, CMD)?;
    let effects: Vec<Value> = s
        .project
        .source_effects_of(media)
        .iter()
        .enumerate()
        .map(|(i, e)| {
            json!({
                "index": i,
                "effect": e.effect,
                "name": e.def().map(|d| d.name),
                "enabled": e.enabled,
                "masks": e.masks.len(),
                "allowed": filmcraft_render::effects::source_effect_refusal(&e.effect).is_none(),
                "params": e.params.iter().map(|(k, p)| (k.clone(), json!({"value": serde_json::to_value(&p.value).unwrap_or_default(), "keyframes": p.keyframes.iter().map(|k| k.time.0).collect::<Vec<_>>()}))).collect::<serde_json::Map<_, _>>(),
            })
        })
        .collect();
    Ok(json!({"item": media.0, "effects": effects}))
}
