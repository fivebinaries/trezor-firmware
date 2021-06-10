use core::iter::{Enumerate, Peekable};
use core::slice;

use crate::ui::{
    display,
    display::Font,
    math::{Color, Offset, Point, Rect},
};

#[derive(Copy, Clone)]
pub enum LineBreaking {
    /// Break line only at whitespace, if possible. If we don't find any
    /// whitespace, break words.
    BreakAtWhitespace,
    /// Break words, adding a hyphen before the line-break. Does not use any
    /// smart algorithm, just char-by-char.
    BreakWordsAndInsertHyphen,
}

#[derive(Copy, Clone)]
pub enum PageBreaking {
    /// Stop after hitting the bottom-right edge of the bounds.
    Cut,
    /// Before stopping at the bottom-right edge, insert ellipsis to signify
    /// more content is available, but only if no hyphen has been inserted yet.
    CutAndInsertEllipsis,
}

/// Visual instructions for laying out a formatted block of text.
#[derive(Copy, Clone)]
pub struct TextStyle {
    /// Bounding box restricting the layout dimensions.
    bounds: Rect,

    /// Background color.
    background_color: Color,
    /// Text color. Can be overridden by `Op::Color`.
    text_color: Color,
    /// Text font ID. Can be overridden by `Op::Font`.
    text_font: Font,

    /// Specifies which line-breaking strategy to use.
    line_breaking: LineBreaking,
    /// Font used for drawing the word-breaking hyphen.
    hyphen_font: Font,
    /// Foreground color used for drawing the hyphen.
    hyphen_color: Color,

    /// Specifies what to do at the end of the page.
    page_breaking: PageBreaking,
    /// Font used for drawing the ellipsis.
    ellipsis_font: Font,
    /// Foreground color used for drawing the ellipsis.
    ellipsis_color: Color,
}

impl TextStyle {
    pub fn render_format<'a>(self, format: &'a str, arg_to_op: impl Fn(&[u8]) -> Option<Op<'a>>) {
        let mut cursor = self.bounds.top_left();

        self.layout_ops(
            &mut Tokenizer::new(format).into_ops(arg_to_op),
            &mut cursor,
            &mut TextRenderer,
        );
    }

    pub fn layout_ops<'a>(
        mut self,
        ops: &mut dyn Iterator<Item = Op<'a>>,
        cursor: &mut Point,
        sink: &mut dyn LayoutSink,
    ) -> LayoutResult {
        for op in ops {
            match op {
                Op::Color(color) => {
                    self.text_color = color;
                }
                Op::Font(font) => {
                    self.text_font = font;
                }
                Op::Text(text) => {
                    if let LayoutResult::OutOfBounds = self.layout_text(text, cursor, sink) {
                        return LayoutResult::OutOfBounds;
                    }
                }
            }
        }
        LayoutResult::Fitting
    }

    pub fn layout_text(
        &self,
        text: &[u8],
        cursor: &mut Point,
        sink: &mut dyn LayoutSink,
    ) -> LayoutResult {
        let mut remaining_text = text;

        while !remaining_text.is_empty() {
            let span = Span::fit_horizontally(
                remaining_text,
                self.bounds.x1 - cursor.x,
                self.text_font,
                self.hyphen_font,
                self.line_breaking,
            );

            // Report the span at the cursor position.
            sink.text(&cursor, &self, &remaining_text[..span.length]);

            // Continue with the rest of the remaining_text.
            remaining_text = &remaining_text[span.length + span.skip_next_chars..];

            // Advance the cursor horizontally.
            cursor.x += span.advance.x;

            if span.advance.y > 0 {
                // We're advancing to the next line.

                // Check if we should be appending a hyphen at this point.
                if span.insert_hyphen_before_line_break {
                    sink.hyphen(&cursor, &self);
                }
                // Check the amount of vertical space we have left.
                if cursor.y + span.advance.y > self.bounds.y1 {
                    if !remaining_text.is_empty() {
                        // Append ellipsis to indicate more content is available, but only if we
                        // haven't already appended a hyphen.
                        let should_append_ellipsis =
                            matches!(self.page_breaking, PageBreaking::CutAndInsertEllipsis)
                                && !span.insert_hyphen_before_line_break;
                        if should_append_ellipsis {
                            sink.ellipsis(&cursor, &self);
                        }
                        // TODO: This does not work in case we are the last
                        // fitting text token on the line, with more text tokens
                        // following and `text.is_empty() == true`.
                    }

                    // Report we are out of bounds and quit.
                    sink.out_of_bounds();

                    return LayoutResult::OutOfBounds;
                } else {
                    // Advance the cursor to the beginning of the next line.
                    cursor.x = self.bounds.x0;
                    cursor.y += span.advance.y;
                }
            }
        }

        LayoutResult::Fitting
    }
}

pub enum LayoutResult {
    Fitting,
    OutOfBounds,
}

/// Visitor for text segment operations.
pub trait LayoutSink {
    fn text(&mut self, cursor: &Point, style: &TextStyle, text: &[u8]) {}
    fn hyphen(&mut self, cursor: &Point, style: &TextStyle) {}
    fn ellipsis(&mut self, cursor: &Point, style: &TextStyle) {}
    fn out_of_bounds(&mut self) {}
}

pub struct TextNoop;

impl LayoutSink for TextNoop {}

pub struct TextRenderer;

impl LayoutSink for TextRenderer {
    fn text(&mut self, cursor: &Point, style: &TextStyle, text: &[u8]) {
        display::text(
            *cursor,
            text,
            style.text_font,
            style.text_color,
            style.background_color,
        );
    }

    fn hyphen(&mut self, cursor: &Point, style: &TextStyle) {
        display::text(
            *cursor,
            b"-",
            style.hyphen_font,
            style.hyphen_color,
            style.background_color,
        );
    }

    fn ellipsis(&mut self, cursor: &Point, style: &TextStyle) {
        display::text(
            *cursor,
            b"...",
            style.ellipsis_font,
            style.ellipsis_color,
            style.background_color,
        );
    }
}

#[derive(Copy, Clone)]
pub enum Token<'a> {
    /// Process literal text content.
    Literal(&'a [u8]),
    /// Process argument with specified descriptor.
    Argument(&'a [u8]),
}

/// Processes a format string into an iterator of `Token`s.
///
/// # Example
///
/// ```
/// let parser = Tokenizer::new("Nice to meet {you}, where you been?");
/// assert!(matches!(parser.next(), Some(Token::Literal("Nice to meet "))));
/// assert!(matches!(parser.next(), Some(Token::Argument("you"))));
/// assert!(matches!(parser.next(), Some(Token::Literal(", where you been?"))));
/// ```
pub struct Tokenizer<'a> {
    input: &'a [u8],
    inner: Peekable<Enumerate<slice::Iter<'a, u8>>>,
}

impl<'a> Tokenizer<'a> {
    /// Create a new tokenizer for `format`, returning an iterator.
    pub fn new(format: &'a str) -> Self {
        let input = format.as_bytes();
        Self {
            input,
            inner: input.iter().enumerate().peekable(),
        }
    }

    /// Transform into an `Op` stream. Literal tokens become `Op::Text`,
    /// argument tokens are converted through `arg_to_op` fn.
    pub fn into_ops(
        self,
        arg_to_op: impl Fn(&[u8]) -> Option<Op<'a>>,
    ) -> impl Iterator<Item = Op<'a>> {
        self.filter_map(move |token| match token {
            Token::Literal(literal) => Some(Op::Text(literal)),
            Token::Argument(argument) => arg_to_op(argument),
        })
    }
}

impl<'a> Iterator for Tokenizer<'a> {
    type Item = Token<'a>;

    fn next(&mut self) -> Option<Self::Item> {
        const ASCII_OPEN_BRACE: u8 = 123;
        const ASCII_CLOSED_BRACE: u8 = 125;

        match self.inner.next() {
            // Argument token is starting. Read until we find '}', then parse the content between
            // the braces and return the token. If we encounter the end of string before the closing
            // brace, quit.
            Some((open, &ASCII_OPEN_BRACE)) => loop {
                match self.inner.next() {
                    Some((close, &ASCII_CLOSED_BRACE)) => {
                        break Some(Token::Argument(&self.input[open + 1..close]));
                    }
                    None => {
                        break None;
                    }
                    _ => {}
                }
            },
            // Literal token is starting. Read until we find '{' or the end of string, and return
            // the token. Use `peek()` for matching the opening brace, se we can keep it
            // in the iterator for the above code.
            Some((start, _)) => loop {
                match self.inner.peek() {
                    Some(&(open, &ASCII_OPEN_BRACE)) => {
                        break Some(Token::Literal(&self.input[start..open]));
                    }
                    None => {
                        break Some(Token::Literal(&self.input[start..]));
                    }
                    _ => {
                        self.inner.next();
                    }
                }
            },
            None => None,
        }
    }
}

#[derive(Copy, Clone)]
pub enum Op<'a> {
    /// Render text with current color and font.
    Text(&'a [u8]),
    /// Set current text color.
    Color(Color),
    /// Set currently used font.
    Font(Font),
}

struct Span {
    /// How many characters from the input text this span is laying out.
    length: usize,
    /// How many chars from the input text should we skip before fitting the
    /// next span?
    skip_next_chars: usize,
    /// By how much to offset the cursor after this span. If the vertical offset
    /// is bigger than zero, it means we are breaking the line.
    advance: Offset,
    /// If we are breaking the line, should we insert a hyphen right after this
    /// span to indicate a word-break?
    insert_hyphen_before_line_break: bool,
}

impl Span {
    fn fit_horizontally(
        text: &[u8],
        max_width: i32,
        text_font: Font,
        hyphen_font: Font,
        breaking: LineBreaking,
    ) -> Self {
        const ASCII_LF: u8 = 10;
        const ASCII_CR: u8 = 13;
        const ASCII_SPACE: u8 = 32;
        const ASCII_HYPHEN: u8 = 45;

        fn is_whitespace(ch: u8) -> bool {
            ch == ASCII_SPACE || ch == ASCII_LF || ch == ASCII_CR
        }

        let hyphen_width = hyphen_font.text_width(&[ASCII_HYPHEN]);

        // The span we return in case the line has to break. We mutate it in the
        // possible break points, and its initial value is returned in case no text
        // at all is fitting the constraints: zero length, zero width, full line
        // break.
        let mut line = Self {
            length: 0,
            advance: Offset::new(0, text_font.line_height()),
            insert_hyphen_before_line_break: false,
            skip_next_chars: 0,
        };

        let mut span_width = 0;
        let mut found_any_whitespace = false;

        for i in 0..text.len() {
            let ch = text[i];

            let char_width = text_font.text_width(&[ch]);

            // Consider if we could be breaking the line at this position.
            if is_whitespace(ch) {
                // Break before the whitespace, without hyphen.
                line.length = i;
                line.advance.x = span_width;
                line.insert_hyphen_before_line_break = false;
                line.skip_next_chars = 1;
                if ch == ASCII_CR {
                    // We'll be breaking the line, but advancing the cursor only by a half of the
                    // regular line height.
                    line.advance.y = text_font.line_height() / 2;
                }
                if ch == ASCII_LF || ch == ASCII_CR {
                    // End of line, break immediately.
                    return line;
                }
                found_any_whitespace = true;
            } else if span_width + char_width > max_width {
                // Return the last breakpoint.
                return line;
            } else {
                let have_space_for_break = span_width + char_width + hyphen_width <= max_width;
                let can_break_word = matches!(breaking, LineBreaking::BreakWordsAndInsertHyphen)
                    || !found_any_whitespace;
                if have_space_for_break && can_break_word {
                    // Break after this character, append hyphen.
                    line.length = i + 1;
                    line.advance.x = span_width + char_width;
                    line.insert_hyphen_before_line_break = true;
                    line.skip_next_chars = 0;
                }
            }

            span_width += char_width;
        }

        // The whole text is fitting.
        Self {
            length: text.len(),
            advance: Offset::new(span_width, 0),
            insert_hyphen_before_line_break: false,
            skip_next_chars: 0,
        }
    }
}
