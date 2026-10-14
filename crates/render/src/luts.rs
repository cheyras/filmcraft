//! LUT resolution for Lumetri (Input LUT, Creative Look) and the built-in LUTs.
//!
//! Lumetri stores a LUT reference as text:
//! * `""` — none;
//! * `lib:<id>` — a LUT in the project's LUT library (`Project::luts`, imported `.cube`/`.3dl`);
//! * `builtin:<id>` — a LUT FilmCraft generates from code: camera log → Rec. 709 conversions
//!   (from the published curves in `filmcraft-color`; input is video-range normalised, as camera
//!   LUTs expect) and the procedural Creative looks.
//!
//! Parsed LUTs are cached by content, so a library LUT is parsed once however many clips use it.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock};

use filmcraft_color::{ColorSpace, Lut, Lut3d, LutFormat, Range};
use filmcraft_project::Project;

/// A built-in LUT: id, label, and how it is made.
pub struct Builtin {
    pub id: String,
    pub label: String,
    kind: BuiltinKind,
}

#[derive(Clone, Copy)]
enum BuiltinKind {
    LogTo709(ColorSpace),
    Look(u32),
}

/// Built-in LUTs offered in the Input LUT menu (camera conversions) and the Look menu.
pub fn builtins() -> &'static [Builtin] {
    static B: OnceLock<Vec<Builtin>> = OnceLock::new();
    B.get_or_init(|| {
        let mut v: Vec<Builtin> = ColorSpace::ALL
            .into_iter()
            .filter(|c| c.is_log())
            .map(|c| Builtin { id: format!("{}-to-rec709", c.id()), label: format!("{} to Rec. 709", c.label()), kind: BuiltinKind::LogTo709(c) })
            .collect();
        let looks = filmcraft_project::find_effect("lumetri")
            .and_then(|d| d.param("look"))
            .and_then(|p| if let filmcraft_project::ParamKind::Choice(o) = p.kind { Some(o) } else { None })
            .unwrap_or(&[]);
        for (i, name) in looks.iter().enumerate().skip(1) {
            v.push(Builtin { id: format!("look-{}", slug(name)), label: (*name).to_string(), kind: BuiltinKind::Look(i as u32) });
        }
        v
    })
}

/// The camera conversions only (Input LUT menu).
pub fn input_builtins() -> impl Iterator<Item = &'static Builtin> {
    builtins().iter().filter(|b| matches!(b.kind, BuiltinKind::LogTo709(_)))
}

fn slug(s: &str) -> String {
    let mut o = String::new();
    for c in s.chars() {
        if c.is_ascii_alphanumeric() {
            o.push(c.to_ascii_lowercase());
        } else if !o.ends_with('-') {
            o.push('-');
        }
    }
    o.trim_matches('-').to_string()
}

impl Builtin {
    /// Generate the LUT (33³; our own procedural work).
    pub fn generate(&self) -> Lut {
        let mut cube = match self.kind {
            BuiltinKind::LogTo709(cs) => {
                let t = filmcraft_color::InputTransform::new(cs, Range::Limited, &filmcraft_color::ColorPipeline::REC709, None);
                Lut3d::from_fn(33, |c| t.convert(c).map(|v| filmcraft_color::linear_to_srgb(v.clamp(0.0, 1.0))))
            }
            BuiltinKind::Look(n) => Lut3d::from_fn(33, |c| crate::effects::apply_look(n, c).map(|v| v.clamp(0.0, 1.0))),
        };
        cube.title = format!("FilmCraft {}", self.label);
        Lut::from_cube(cube)
    }
}

fn cache() -> &'static Mutex<HashMap<String, Arc<Lut>>> {
    static C: OnceLock<Mutex<HashMap<String, Arc<Lut>>>> = OnceLock::new();
    C.get_or_init(Default::default)
}

fn cached(key: String, make: impl FnOnce() -> Option<Lut>) -> Option<Arc<Lut>> {
    if let Some(l) = cache().lock().unwrap_or_else(|e| e.into_inner()).get(&key) {
        return Some(l.clone());
    }
    let l = Arc::new(make()?);
    let mut c = cache().lock().unwrap_or_else(|e| e.into_inner());
    if c.len() > 64 {
        c.clear();
    }
    c.insert(key, l.clone());
    Some(l)
}

/// Resolve a Lumetri LUT reference (see module docs). `None` for "" or unknown references.
pub fn resolve(project: Option<&Project>, spec: &str) -> Option<Arc<Lut>> {
    let spec = spec.trim();
    if let Some(id) = spec.strip_prefix("builtin:") {
        let b = builtins().iter().find(|b| b.id == id)?;
        return cached(format!("builtin:{id}"), || Some(b.generate()));
    }
    let id = spec.strip_prefix("lib:")?;
    let entry = project?.luts.iter().find(|l| l.id == id)?;
    let key = format!("lib:{:016x}", hash(&entry.text));
    let fmt = LutFormat::from_path(&format!("x.{}", entry.format));
    cached(key, || Lut::parse(&entry.text, fmt).ok())
}

/// Whether `spec` names a LUT that exists: a built-in, or an entry of the project's library. Cheap
/// (no parsing, no hashing): for deciding which render path a clip takes. [`resolve`] can still
/// fail on a library entry that does not parse.
pub fn exists(project: Option<&Project>, spec: &str) -> bool {
    let spec = spec.trim();
    if let Some(id) = spec.strip_prefix("builtin:") {
        return builtins().iter().any(|b| b.id == id);
    }
    spec.strip_prefix("lib:").is_some_and(|id| project.is_some_and(|p| p.luts.iter().any(|l| l.id == id)))
}

/// A key naming what the LUT reference `spec` resolves to: it changes when a library LUT's content
/// changes, not only when the reference does. `None` for "" and references that do not resolve.
/// For cache keys and hashes that cover pictures rendered through a LUT.
pub fn content_key(project: Option<&Project>, spec: &str) -> Option<String> {
    let spec = spec.trim();
    if let Some(id) = spec.strip_prefix("builtin:") {
        return builtins().iter().any(|b| b.id == id).then(|| format!("builtin:{id}"));
    }
    let id = spec.strip_prefix("lib:")?;
    let entry = project?.luts.iter().find(|l| l.id == id)?;
    Some(format!("lib:{:016x}:{}", hash(&entry.text), entry.format))
}

fn hash(s: &str) -> u64 {
    // FNV-1a
    let mut h = 0xcbf2_9ce4_8422_2325u64;
    for b in s.as_bytes() {
        h ^= *b as u64;
        h = h.wrapping_mul(0x100_0000_01b3);
    }
    h ^ s.len() as u64
}

/// Human-readable name of a LUT reference (for panels).
pub fn label(project: Option<&Project>, spec: &str) -> String {
    if spec.is_empty() {
        return "None".into();
    }
    if let Some(id) = spec.strip_prefix("builtin:") {
        return builtins().iter().find(|b| b.id == id).map(|b| b.label.clone()).unwrap_or_else(|| id.to_string());
    }
    if let Some(id) = spec.strip_prefix("lib:") {
        return project.and_then(|p| p.luts.iter().find(|l| l.id == id)).map(|l| l.name.clone()).unwrap_or_else(|| format!("{id} (missing)"));
    }
    spec.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builtins_generate_and_resolve() {
        assert!(input_builtins().count() >= 9);
        let b = builtins().iter().find(|b| b.id == "slog3-sgamut3cine-to-rec709").unwrap();
        let l = b.generate();
        // S-Log3 mid grey (code 420; the LUT sees video-range normalised values like the rest of
        // Lumetri) → ≈ 18 % linear on the sRGB curve
        let g = l.apply([(420.0 - 64.0) / 876.0; 3]);
        assert!((g[0] - filmcraft_color::linear_to_srgb(0.18)).abs() < 0.03, "{g:?}");
        assert!(resolve(None, "builtin:slog3-sgamut3cine-to-rec709").is_some());
        assert!(resolve(None, "builtin:look-teal-orange").is_some());
        assert!(resolve(None, "").is_none() && resolve(None, "lib:nope").is_none());
        // a look LUT reproduces the procedural look
        let lk = resolve(None, "builtin:look-teal-orange").unwrap();
        let c = [0.3, 0.5, 0.7];
        let (a, want) = (lk.apply(c), crate::effects::apply_look(1, c));
        assert!((0..3).all(|k| (a[k] - want[k]).abs() < 0.01), "{a:?} vs {want:?}");
    }

    #[test]
    fn references_report_existence_and_content() {
        let mut p = Project::new("t");
        let swap = |c: [f32; 3]| [c[2], c[1], c[0]];
        let entry = |f: &dyn Fn([f32; 3]) -> [f32; 3]| filmcraft_project::ProjectLut {
            id: "a".into(),
            name: "A".into(),
            source_path: None,
            format: "cube".into(),
            text: Lut::from_cube(Lut3d::from_fn(5, f)).to_cube().as_str().into(),
        };
        p.luts.push(entry(&swap));
        let q = Some(&p);
        assert!(exists(q, "lib:a") && exists(q, " lib:a ") && exists(None, "builtin:slog3-sgamut3cine-to-rec709"));
        assert!(!exists(q, "lib:b") && !exists(None, "lib:a") && !exists(q, "") && !exists(q, "builtin:nope") && !exists(q, "a") && !exists(q, "lib:"));
        let key = content_key(q, "lib:a");
        assert!(key.is_some() && content_key(q, "lib:b").is_none() && content_key(None, "lib:a").is_none());
        assert_eq!(content_key(None, "builtin:look-teal-orange").as_deref(), Some("builtin:look-teal-orange"));
        assert_eq!(content_key(None, "builtin:nope"), None);
        // the same id with other content is another key
        p.luts[0] = entry(&|c| c);
        assert_ne!(content_key(Some(&p), "lib:a"), key);
    }

    #[test]
    fn library_luts_resolve_by_content() {
        let mut p = Project::new("t");
        let text = Lut::from_cube(Lut3d::from_fn(5, |c| [c[2], c[1], c[0]])).to_cube();
        p.luts.push(filmcraft_project::ProjectLut { id: "lut1".into(), name: "Swap".into(), source_path: None, format: "cube".into(), text: text.into() });
        let l = resolve(Some(&p), "lib:lut1").unwrap();
        assert_eq!(l.apply([1.0, 0.0, 0.0]), [0.0, 0.0, 1.0]);
        assert_eq!(label(Some(&p), "lib:lut1"), "Swap");
        assert_eq!(label(Some(&p), "lib:x"), "x (missing)");
    }
}
