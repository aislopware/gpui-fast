//! Tests that a line which fits its wrap width is left alone without looking
//! for its breaks, and that one a hair too wide still breaks where UAX #14
//! allows. See [`crate::fast::line_breaks`].

use crate::{
    Boundary, IndentAdjustment, LineFragment, LineWrapper, Pixels, SharedString, TestAppContext,
    TextRun, black, font, px,
};

fn wrapper(cx: &TestAppContext) -> LineWrapper {
    let font_id = cx.text_system().resolve_font(&font(".ZedMono"));
    LineWrapper::new(font_id, px(16.), cx.text_system().clone())
}

fn width_of(wrapper: &mut LineWrapper, fragments: &[LineFragment]) -> Pixels {
    let mut width = px(0.);
    for fragment in fragments {
        match fragment {
            LineFragment::Text { text } => {
                for character in text.chars().filter(|&character| character != '\n') {
                    width += wrapper.width_for_char(character);
                }
            }
            LineFragment::Element {
                width: element_width,
                ..
            } => width += *element_width,
        }
    }
    width
}

fn wrap(
    wrapper: &mut LineWrapper,
    fragments: &[LineFragment],
    wrap_width: Pixels,
) -> Vec<Boundary> {
    wrapper
        .wrap_line(fragments, wrap_width, IndentAdjustment::NoIndent)
        .collect()
}

#[test]
fn a_wrapped_line_that_fits_exactly_is_not_broken() {
    let cx = TestAppContext::single();
    let mut wrapper = wrapper(&cx);
    let cases: [(&[LineFragment], usize); 3] = [
        (&[LineFragment::text("aaa foo/bar")], 8),
        (
            &[
                LineFragment::text("aaa "),
                LineFragment::element(px(30.), 1),
                LineFragment::text(" bb"),
            ],
            6,
        ),
        (&[LineFragment::text("aaa\nbbb ccc")], 8),
    ];
    for (fragments, break_at) in cases {
        let width = width_of(&mut wrapper, fragments);
        assert!(
            crate::fast::line_breaks::unless_fragments_fit(
                &mut wrapper,
                fragments,
                &joined(fragments),
                width
            )
            .is_empty()
        );
        assert_eq!(
            wrap(&mut wrapper, fragments, width),
            [],
            "{:?}",
            joined(fragments)
        );
        assert_eq!(
            wrap(&mut wrapper, fragments, width - px(0.01)),
            [Boundary {
                ix: break_at,
                next_indent: 0
            }],
            "{:?}",
            joined(fragments),
        );
    }
}

fn joined(fragments: &[LineFragment]) -> String {
    fragments
        .iter()
        .map(|fragment| match fragment {
            LineFragment::Text { text } => text.to_string(),
            LineFragment::Element { len_utf8, .. } => "a".repeat(*len_utf8),
        })
        .collect()
}

#[test]
fn a_shaped_line_that_fits_exactly_is_not_broken() {
    let mut cx = TestAppContext::single();
    let cx = cx.add_empty_window();
    cx.update(|window, _| {
        let text = SharedString::from("aaa foo/bar");
        let runs = [TextRun {
            len: text.len(),
            font: font(".ZedMono"),
            color: black(),
            background_color: None,
            underline: None,
            strikethrough: None,
        }];
        let text_system = window.text_system();
        let width = text_system
            .shape_line(text.clone(), px(16.), &runs, None)
            .width;
        let rows = |wrap_width: Pixels| {
            text_system
                .shape_text(text.clone(), px(16.), &runs, Some(wrap_width), None)
                .unwrap()
                .iter()
                .map(|line| {
                    let mut starts = line
                        .wrap_boundaries()
                        .iter()
                        .map(|boundary| {
                            line.unwrapped_layout.runs[boundary.run_ix].glyphs[boundary.glyph_ix]
                                .index
                        })
                        .collect::<Vec<_>>();
                    starts.insert(0, 0);
                    starts
                })
                .collect::<Vec<_>>()
        };
        assert_eq!(rows(width), [[0]]);
        assert_eq!(rows(width - px(0.01)), [[0, 8]]);
    });
}

/// ASCII read as Latin-1 breaks where the same text read as UTF-8 does.
#[test]
fn ascii_breaks_as_its_utf8_does() {
    let mut state = 0x9e37_79b9_7f4a_7c15_u64;
    let mut next = move || {
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        state
    };
    let alphabet: Vec<char> = (0u8..128).map(char::from).collect();
    for _ in 0..20_000 {
        let len = 1 + (next() % 24) as usize;
        let text: String = (0..len)
            .map(|_| alphabet[(next() % 128) as usize])
            .collect();
        assert_eq!(
            crate::fast::line_breaks::break_indices(&text),
            LineWrapper::break_indices(&text).collect::<Vec<_>>(),
            "{text:?}",
        );
    }
}
