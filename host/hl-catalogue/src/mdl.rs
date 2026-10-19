//! Studio model reader: sequence labels, events and blending, straight from
//! the player's own `.mdl` (version 10).

use std::path::Path;

pub struct Event {
    pub frame: i32,
    pub id: i32,
    pub options: String,
}

pub struct Seq {
    pub index: usize,
    pub label: String,
    pub fps: f32,
    pub frames: i32,
    pub activity: i32,
    pub blends: i32,
    pub events: Vec<Event>,
}

#[derive(Default)]
pub struct Model {
    pub seqs: Vec<Seq>,
    pub bone_controllers: Vec<(i32, i32)>,
}

fn i32le(d: &[u8], o: usize) -> Option<i32> {
    Some(i32::from_le_bytes(d.get(o..o + 4)?.try_into().ok()?))
}

fn f32le(d: &[u8], o: usize) -> Option<f32> {
    Some(f32::from_le_bytes(d.get(o..o + 4)?.try_into().ok()?))
}

fn cstr(b: &[u8]) -> String {
    let end = b.iter().position(|&c| c == 0).unwrap_or(b.len());
    String::from_utf8_lossy(&b[..end]).to_string()
}

pub fn load(path: &Path) -> Result<Model, String> {
    let d = std::fs::read(path).map_err(|e| format!("{}: {e}", path.display()))?;
    parse(&d).ok_or_else(|| format!("{}: not a version 10 studio model", path.display()))
}

pub fn parse(d: &[u8]) -> Option<Model> {
    if d.get(0..4)? != b"IDST" {
        return None;
    }
    let bc_n = i32le(d, 148)? as usize;
    let bc_o = i32le(d, 152)? as usize;
    let n = i32le(d, 164)? as usize;
    let t = i32le(d, 168)? as usize;
    let mut m = Model::default();
    for c in 0..bc_n {
        let o = bc_o + c * 24;
        // mstudiobonecontroller_t: bone, type, start, end, rest, index
        m.bone_controllers.push((i32le(d, o + 4)?, i32le(d, o + 20)?));
    }
    for s in 0..n {
        let o = t + s * 176;
        let ne = i32le(d, o + 48)? as usize;
        let eo = i32le(d, o + 52)? as usize;
        let mut events = Vec::new();
        for e in 0..ne {
            let p = eo + e * 76;
            events.push(Event {
                frame: i32le(d, p)?,
                id: i32le(d, p + 4)?,
                options: cstr(d.get(p + 12..p + 76)?),
            });
        }
        m.seqs.push(Seq {
            index: s,
            label: cstr(d.get(o..o + 32)?),
            fps: f32le(d, o + 32)?,
            frames: i32le(d, o + 56)?,
            activity: i32le(d, o + 40)?,
            blends: i32le(d, o + 120)?,
            events,
        });
    }
    Some(m)
}
