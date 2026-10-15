//! Small transport glyphs (play, pause, previous, next) drawn in code as RGBA pixels, for the Windows taskbar
//! thumbnail toolbar (src/taskbar.rs). White shapes on a transparent background, 4×4 supersampled edges.

/// The glyphs the toolbar shows.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Glyph {
    Play,
    Pause,
    Prev,
    Next,
}

/// Polygons in a 0…1 box (x right, y down).
fn shapes(g: Glyph) -> Vec<Vec<(f32, f32)>> {
    let rect = |x0: f32, y0: f32, x1: f32, y1: f32| vec![(x0, y0), (x1, y0), (x1, y1), (x0, y1)];
    match g {
        Glyph::Play => vec![vec![(0.28, 0.16), (0.84, 0.5), (0.28, 0.84)]],
        Glyph::Pause => vec![rect(0.24, 0.18, 0.42, 0.82), rect(0.58, 0.18, 0.76, 0.82)],
        Glyph::Prev => vec![rect(0.18, 0.2, 0.3, 0.8), vec![(0.82, 0.2), (0.82, 0.8), (0.32, 0.5)]],
        Glyph::Next => vec![vec![(0.18, 0.2), (0.68, 0.5), (0.18, 0.8)], rect(0.7, 0.2, 0.82, 0.8)],
    }
}

/// Even-odd point-in-polygon test.
fn inside(poly: &[(f32, f32)], x: f32, y: f32) -> bool {
    let mut c = false;
    let mut j = poly.len() - 1;
    for i in 0..poly.len() {
        let (xi, yi) = poly[i];
        let (xj, yj) = poly[j];
        if (yi > y) != (yj > y) && x < (xj - xi) * (y - yi) / (yj - yi) + xi {
            c = !c;
        }
        j = i;
    }
    c
}

/// `size`×`size` pixels, 4 bytes each, top row first, in R, G, B, A order (straight alpha).
pub fn rgba(g: Glyph, size: u32) -> Vec<u8> {
    const SS: u32 = 4;
    let polys = shapes(g);
    let mut out = Vec::with_capacity((size * size * 4) as usize);
    for py in 0..size {
        for px in 0..size {
            let mut hits = 0u32;
            for sy in 0..SS {
                for sx in 0..SS {
                    let x = (px as f32 + (sx as f32 + 0.5) / SS as f32) / size as f32;
                    let y = (py as f32 + (sy as f32 + 0.5) / SS as f32) / size as f32;
                    if polys.iter().any(|p| inside(p, x, y)) {
                        hits += 1;
                    }
                }
            }
            let a = (hits * 255 / (SS * SS)) as u8;
            out.extend_from_slice(&[255, 255, 255, a]);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn alpha(px: &[u8], size: u32, x: u32, y: u32) -> u8 {
        px[((y * size + x) * 4 + 3) as usize]
    }

    #[test]
    fn glyphs_have_the_right_size_and_shape() {
        for g in [Glyph::Play, Glyph::Pause, Glyph::Prev, Glyph::Next] {
            let px = rgba(g, 16);
            assert_eq!(px.len(), 16 * 16 * 4);
            assert_eq!(alpha(&px, 16, 0, 0), 0, "{g:?}: corners stay transparent");
            assert!(px.chunks(4).any(|p| p[3] == 255), "{g:?}: something is drawn");
        }
        let play = rgba(Glyph::Play, 20);
        assert_eq!(alpha(&play, 20, 9, 10), 255, "the middle of the triangle is solid");
        let pause = rgba(Glyph::Pause, 20);
        assert_eq!(alpha(&pause, 20, 10, 10), 0, "the gap between the pause bars is clear");
        assert_eq!(alpha(&pause, 20, 6, 10), 255);
    }
}
