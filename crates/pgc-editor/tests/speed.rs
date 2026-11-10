//! Speed check with a very large SQL text (CLAUDE.md Phase 5). Not part of the normal
//! test run:
//!
//! ```text
//! cargo test --release -p pgc-editor --test speed -- --ignored --nocapture
//! ```

use std::time::Instant;

use pgc_editor::{Action, Editor, EditorStyles, Motion, render};
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;

fn big_sql(bytes: usize) -> String {
    let block = "-- a comment line\n\
                 CREATE TABLE IF NOT EXISTS public.t (id integer NOT NULL, name text DEFAULT 'x''y');\n\
                 INSERT INTO public.t VALUES (1, E'a\\'b'), (2, $$dollar\nquoted$$);\n\
                 /* block\n comment */ SELECT \"Quoted\", 3.5e-2 FROM public.t WHERE id = $1;\n";
    let mut s = String::with_capacity(bytes + block.len());
    while s.len() < bytes {
        s.push_str(block);
    }
    s
}

fn time<T>(what: &str, f: impl FnOnce() -> T) -> T {
    let t = Instant::now();
    let r = f();
    println!("{what:<45} {:>8.1} ms", t.elapsed().as_secs_f64() * 1000.0);
    r
}

#[test]
#[ignore]
fn speed_100_mb() {
    let text = time("build 100 MB of SQL", || big_sql(100 * 1024 * 1024));
    let mut e = time("load into the rope", || Editor::new(&text, false, 4));
    drop(text);
    println!("lines: {}", e.len_lines());
    e.set_view(50, 200);
    let area = Rect::new(0, 0, 210, 50);
    let styles = EditorStyles::default();
    let mut buf = Buffer::empty(area);

    time("first frame at the top", || {
        render(&e, area, &mut buf, &styles, true)
    });
    time("jump to the end (cold cache) + frame", || {
        e.apply(Action::Move(Motion::DocEnd, false));
        render(&e, area, &mut buf, &styles, true)
    });
    time("background scan of the rest", || {
        while e.needs_scan() {
            e.background_scan(50_000);
        }
    });
    time("frame at the end (warm cache)", || {
        render(&e, area, &mut buf, &styles, true)
    });
    let mid = e.len_lines() / 2;
    time("jump to the middle, type 100 chars + frame", || {
        e.set_cursor(e.line_to_char(mid), false);
        for _ in 0..100 {
            e.apply(Action::Insert("x".into()));
        }
        render(&e, area, &mut buf, &styles, true)
    });
    time("page down 100 times with frames", || {
        for _ in 0..100 {
            e.apply(Action::Move(Motion::PageDown, false));
            render(&e, area, &mut buf, &styles, true);
        }
    });
    time("undo 100 typing steps", || {
        for _ in 0..100 {
            e.apply(Action::Undo);
        }
    });
}
