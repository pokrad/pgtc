//! Line and block characters of the pictures (README and help) as shapes, not as
//! glyphs of the font: a glyph of `│` or `█` is shorter than the row, so frames, grid
//! lines and sparklines would break between the rows of the picture. A terminal
//! stretches them over the whole cell; these shapes do the same.

/// Thickness of a light line, and of each of the two lines of a double one.
const LIGHT: f32 = 1.5;
const DOUBLE: f32 = 1.2;
/// Distance of each line of a double line from the middle of the cell.
const APART: f32 = 2.0;

/// A line in cell coordinates: from (`x0`, `y0`) to (`x1`, `y1`) — horizontal or
/// vertical —, `t` thick. 0 and 1 are the cell's edges, `M` its middle; `-` and `+`
/// a double line's two lines on either side of it.
#[derive(Clone, Copy)]
enum P {
    /// An edge of the cell.
    E0,
    E1,
    /// The middle, and the two lines of a double line around it.
    M,
    Minus,
    Plus,
}

/// One line of a box character: from x, from y, to x, to y, double?
type Line = (P, P, P, P, bool);

/// The lines of a box character.
fn lines(c: char) -> Option<&'static [Line]> {
    use P::{E0, E1, M, Minus as N, Plus as Q};
    Some(match c {
        // light lines: from the middle to the edges they go to
        '─' => &[(E0, M, E1, M, false)],
        '│' => &[(M, E0, M, E1, false)],
        '┌' => &[(M, M, E1, M, false), (M, M, M, E1, false)],
        '┐' => &[(E0, M, M, M, false), (M, M, M, E1, false)],
        '└' => &[(M, E0, M, M, false), (M, M, E1, M, false)],
        '┘' => &[(M, E0, M, M, false), (E0, M, M, M, false)],
        '├' => &[(M, E0, M, E1, false), (M, M, E1, M, false)],
        '┤' => &[(M, E0, M, E1, false), (E0, M, M, M, false)],
        '┬' => &[(E0, M, E1, M, false), (M, M, M, E1, false)],
        '┴' => &[(E0, M, E1, M, false), (M, E0, M, M, false)],
        '┼' => &[(E0, M, E1, M, false), (M, E0, M, E1, false)],
        // double lines: the outer line of a corner goes round the inner one
        '═' => &[(E0, N, E1, N, true), (E0, Q, E1, Q, true)],
        '║' => &[(N, E0, N, E1, true), (Q, E0, Q, E1, true)],
        '╔' => &[
            (N, N, E1, N, true),
            (N, N, N, E1, true),
            (Q, Q, E1, Q, true),
            (Q, Q, Q, E1, true),
        ],
        '╗' => &[
            (E0, N, Q, N, true),
            (Q, N, Q, E1, true),
            (E0, Q, N, Q, true),
            (N, Q, N, E1, true),
        ],
        '╚' => &[
            (N, E0, N, Q, true),
            (N, Q, E1, Q, true),
            (Q, E0, Q, N, true),
            (Q, N, E1, N, true),
        ],
        '╝' => &[
            (Q, E0, Q, Q, true),
            (E0, Q, Q, Q, true),
            (N, E0, N, N, true),
            (E0, N, N, N, true),
        ],
        // a double vertical line with a light line to one side
        '╟' => &[
            (N, E0, N, E1, true),
            (Q, E0, Q, E1, true),
            (Q, M, E1, M, false),
        ],
        '╢' => &[
            (N, E0, N, E1, true),
            (Q, E0, Q, E1, true),
            (E0, M, N, M, false),
        ],
        _ => return None,
    })
}

/// `sym` is a horizontal line: a run of it is one shape over all its cells.
pub(super) fn stretches(sym: &str) -> bool {
    matches!(sym, "─" | "═")
}

/// SVG shapes for character `sym` in the cell at (`x`, `y`), `cw` × `ch` pixels, in
/// colour `fill`; `None` when it is drawn as text.
pub(super) fn cell_shapes(
    sym: &str,
    x: f32,
    y: f32,
    cw: f32,
    ch: f32,
    fill: &str,
) -> Option<String> {
    let mut chars = sym.chars();
    let (Some(c), None) = (chars.next(), chars.next()) else {
        return None;
    };
    let rect = |rx: f32, ry: f32, w: f32, h: f32| {
        format!("<rect x=\"{rx}\" y=\"{ry}\" width=\"{w}\" height=\"{h}\" fill=\"{fill}\"/>\n")
    };
    // block elements: the lower eighths and the full block (sparklines); a little
    // wider and taller, so that no gap is left to the cell beside or below
    let eighths = match c {
        '▁'..='▇' => Some(c as u32 - '▁' as u32 + 1),
        '█' => Some(8),
        _ => None,
    };
    if let Some(n) = eighths {
        let h = ch * n as f32 / 8.0;
        return Some(rect(x, y + ch - h, cw + 0.5, h + 0.5));
    }
    let at = |p: P, start: f32, size: f32| match p {
        P::E0 => start,
        P::E1 => start + size,
        P::M => start + size / 2.0,
        P::Minus => start + size / 2.0 - APART,
        P::Plus => start + size / 2.0 + APART,
    };
    let mut out = String::new();
    for &(px0, py0, px1, py1, double) in lines(c)? {
        let t = if double { DOUBLE } else { LIGHT };
        let (x0, x1) = (at(px0, x, cw), at(px1, x, cw));
        let (y0, y1) = (at(py0, y, ch), at(py1, y, ch));
        // as thick as the line across it, and half that longer at each end — so that
        // two lines meet in a corner — but not out of the cell
        let left = (x0.min(x1) - t / 2.0).max(x);
        let right = (x0.max(x1) + t / 2.0).min(x + cw);
        let top = (y0.min(y1) - t / 2.0).max(y);
        let bottom = (y0.max(y1) + t / 2.0).min(y + ch);
        out.push_str(&rect(left, top, right - left, bottom - top));
    }
    Some(out)
}
