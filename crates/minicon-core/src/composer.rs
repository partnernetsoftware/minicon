//! Pure single-line composer editing rules.
//!
//! Clipboard I/O stays in the host. This module only owns deterministic text
//! and selection changes so the behavior can later converge with the shared
//! frontend without importing terminal or window authority.

pub const PASTE_LIMIT_BYTES: usize = 64 * 1024;
const COMPOSER_LIMIT_BYTES: usize = PASTE_LIMIT_BYTES;

/// Complete state of the external single-line input surface.
///
/// Host adapters may inspect these fields for rendering and routing, while
/// transitions that must clear several fields stay atomic here.
#[derive(Debug, Default, Eq, PartialEq)]
pub struct ComposerState {
    pub text: String,
    pub preedit: String,
    pub focused: bool,
    pub select_all: bool,
    pub submit_error: Option<String>,
    /// Where the next edit lands, as a byte offset into `text`.
    ///
    /// This widget used to have no caret at all: editing appended and deleted
    /// at the end, so text that scrolled out of view was not merely hidden but
    /// unreachable — there was nothing to move. Every invariant here exists
    /// because the offset is a byte index into UTF-8: it is kept on a
    /// character boundary and never past the end, so slicing on it cannot
    /// panic.
    pub caret: usize,
}

impl ComposerState {
    /// Moves the caret onto a character boundary and returns it.
    ///
    /// Both halves matter. The caret is a byte offset that outlives the text
    /// it pointed into -- the accessibility bus can replace the contents
    /// underneath it -- so every edit clamps first, and stores the result so
    /// that an early return cannot leave a stale offset behind for the next
    /// slice to panic on.
    fn clamped_caret(&mut self) -> usize {
        self.caret = clamp_caret(&self.text, self.caret);
        self.caret
    }

    pub fn cancel_focus(&mut self) {
        self.focused = false;
        self.preedit.clear();
        self.select_all = false;
        self.submit_error = None;
    }

    /// Returns one terminal submission and resets all transient edit state.
    pub fn take_submission(&mut self) -> Option<String> {
        let mut submission = (!self.text.is_empty()).then(|| std::mem::take(&mut self.text));
        if let Some(text) = submission.as_mut() {
            text.push('\r');
        }
        self.text.clear();
        self.preedit.clear();
        self.select_all = false;
        self.submit_error = None;
        submission
    }

    pub fn restore_failed_submission(&mut self, mut submission: String, error: String) {
        if submission.ends_with('\r') {
            submission.pop();
        }
        self.text = submission;
        self.preedit.clear();
        self.select_all = false;
        self.focused = true;
        self.submit_error = Some(error);
    }
}

/// Where the painted window into a single-line composer begins.
///
/// The composer stays one line on purpose: its content is submitted to a
/// shell, where a newline means "run", so wrapping would misrepresent what
/// pressing Enter does. What is *not* on purpose is painting from the head
/// and clipping the tail, which is what the chrome painter does with any
/// string too wide for its box. Past that width the composer showed stale
/// leading text while every new character — and the caret with them — landed
/// outside the box. That is typing into a surface that cannot show what you
/// typed.
///
/// Every edit happens at the end (there is no caret position to move), so the
/// end is the only span worth showing. These offsets slide the window to keep
/// it there.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct VisibleWindow {
    /// Byte offset into the committed text where painting starts.
    pub text: usize,
    /// Byte offset into the IME preedit where painting starts. Nonzero only
    /// when the preedit alone already fills the box.
    pub preedit: usize,
    /// Whether anything was scrolled out of view, so the painter can say so
    /// rather than letting the text appear to begin where it does not.
    pub truncated: bool,
}

/// The painter's own width rule, so the measurement here and the advance
/// there cannot disagree: a double-width character owns two cells.
fn character_cells(character: char) -> usize {
    if unicode_width::UnicodeWidthChar::width(character).unwrap_or(1) > 1 {
        2
    } else {
        1
    }
}

/// Total painted width of `text`, in cells.
pub fn cells(text: &str) -> usize {
    text.chars().map(character_cells).sum()
}

/// Walks backwards from the end of `text`, returning the byte offset of the
/// first character that still fits in `budget` and charging what it consumed.
fn tail_offset(text: &str, budget: &mut usize) -> usize {
    let mut offset = text.len();
    for (index, character) in text.char_indices().rev() {
        let cells = character_cells(character);
        if cells > *budget {
            break;
        }
        *budget -= cells;
        offset = index;
    }
    offset
}

/// `cells` is the box width in cells; `reserved` is what the caret and any
/// other fixed decoration already claim.
///
/// The window follows the **caret**, not the end of the line. Anchoring to
/// the end was right only while the caret could not move; once it can, a
/// caret moved left has to bring the view with it or the user is editing
/// text they cannot see — the original defect, one step along.
///
/// A truncated window is measured twice: the first pass discovers that
/// something was hidden, the second pays a cell for the marker that says so.
/// Reserving that cell unconditionally would shrink every short line for a
/// marker it will never show.
pub fn visible_window(
    text: &str,
    preedit: &str,
    caret: usize,
    reserved: usize,
    cells: usize,
) -> VisibleWindow {
    let caret = clamp_caret(text, caret);
    let measure = |extra: usize| {
        let budget = cells.saturating_sub(reserved).saturating_sub(extra);
        // Everything from the caret rightwards competes for the same budget,
        // and the caret must stay inside it, so the text before the caret is
        // what gets dropped first.
        let mut trailing = budget;
        let preedit_offset = tail_offset(preedit, &mut trailing);
        let after_caret = if preedit_offset > 0 {
            // The preedit alone fills the box; nothing committed can show.
            0
        } else {
            cells_fitting_forward(&text[caret..], &mut trailing)
        };
        let mut leading = trailing;
        let text_offset = if preedit_offset > 0 {
            text.len()
        } else {
            tail_offset(&text[..caret], &mut leading)
        };
        let _ = after_caret;
        VisibleWindow {
            text: text_offset,
            preedit: preedit_offset,
            truncated: text_offset > 0 || preedit_offset > 0,
        }
    };
    let window = measure(0);
    if window.truncated { measure(1) } else { window }
}

/// Charges `budget` for as much of `text` as fits from its start, returning
/// the cells consumed. Used for the span *after* the caret, which is shown
/// only with what the caret's own context leaves over.
fn cells_fitting_forward(text: &str, budget: &mut usize) -> usize {
    let mut used = 0;
    for character in text.chars() {
        let width = character_cells(character);
        if width > *budget {
            break;
        }
        *budget -= width;
        used += width;
    }
    used
}

/// Where a caret movement should land.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Move {
    Left,
    Right,
    LineStart,
    LineEnd,
}

pub fn select_all(state: &mut ComposerState) {
    state.select_all = !state.text.is_empty();
    // A selection covers everything, so the caret has no meaningful position
    // inside it until the next edit collapses it.
    state.caret = state.text.len();
}

pub fn insert(state: &mut ComposerState, text: &str) {
    prepare_edit(state);
    let remaining = COMPOSER_LIMIT_BYTES.saturating_sub(state.text.len());
    let mut end = remaining.min(text.len());
    while !text.is_char_boundary(end) {
        end = end.saturating_sub(1);
    }
    let addition = &text[..end];
    let caret = state.clamped_caret();
    state.text.insert_str(caret, addition);
    state.caret = caret + addition.len();
}

/// Deletes the character *before* the caret.
pub fn backspace(state: &mut ComposerState) {
    if prepare_edit(state) {
        return;
    }
    // Stored, not just used: an edge case that returns early must still leave
    // the caret on a boundary, or the stale offset outlives this call and the
    // next slice panics.
    let caret = state.clamped_caret();
    let Some(previous) = previous_boundary(&state.text, caret) else {
        return;
    };
    state.text.replace_range(previous..caret, "");
    state.caret = previous;
}

/// Deletes the character *at* the caret, which is what Delete does and what
/// Backspace cannot do once the caret can sit before text.
pub fn delete_forward(state: &mut ComposerState) {
    if prepare_edit(state) {
        return;
    }
    let caret = state.clamped_caret();
    let Some(next) = next_boundary(&state.text, caret) else {
        return;
    };
    state.text.replace_range(caret..next, "");
    state.caret = caret;
}

/// Moves the caret, collapsing any selection first: an arrow key means "put
/// the caret here", not "edit everything".
pub fn move_caret(state: &mut ComposerState, movement: Move) {
    state.select_all = false;
    let caret = state.clamped_caret();
    state.caret = match movement {
        Move::Left => previous_boundary(&state.text, caret).unwrap_or(0),
        Move::Right => next_boundary(&state.text, caret).unwrap_or(state.text.len()),
        Move::LineStart => 0,
        Move::LineEnd => state.text.len(),
    };
}

/// Places the caret at a painted column, for a pointer click.
///
/// `cell` counts from the first *visible* cell, so the caller adds the window
/// offset; a click past the end lands after the last character rather than
/// being ignored, which is what every other text field does.
pub fn caret_at_cell(text: &str, from: usize, cell: usize) -> usize {
    let from = clamp_caret(text, from);
    let mut consumed = 0;
    for (index, character) in text[from..].char_indices() {
        let width = character_cells(character);
        // Past the halfway point of a cell the click belongs to the next
        // character, which is what makes clicking "between" two glyphs land
        // where the pointer visually is.
        if consumed + width > cell {
            return from + index + usize::from(cell >= consumed + width.div_ceil(2)) * character.len_utf8();
        }
        consumed += width;
    }
    text.len()
}

pub fn selected_text<'a>(buffer: &'a str, selected: &bool) -> Option<&'a str> {
    (*selected && !buffer.is_empty()).then_some(buffer)
}

pub fn cut(state: &mut ComposerState) -> Option<String> {
    if state.select_all && !state.text.is_empty() {
        state.select_all = false;
        state.caret = 0;
        return Some(std::mem::take(&mut state.text));
    }
    None
}

pub fn paste(state: &mut ComposerState, text: &str) {
    let normalized = normalize_single_line(text);
    if !normalized.is_empty() {
        insert(state, &normalized);
    }
}

/// Replace the buffer with AT-SPI `SetTextContents` / computed `InsertText`
/// result. Keeps the composer single-line so a bus write cannot inject
/// newlines into the painted input.
pub fn replace_text(state: &mut ComposerState, text: &str) {
    state.select_all = false;
    state.text = normalize_single_line(text);
    state.caret = state.text.len();
}

/// A caret that has drifted past the end, or into the middle of a character,
/// is clamped rather than trusted. Both are reachable: the text can be
/// replaced from the accessibility bus while the caret still refers to the
/// old contents.
fn clamp_caret(text: &str, caret: usize) -> usize {
    let mut caret = caret.min(text.len());
    while !text.is_char_boundary(caret) {
        caret -= 1;
    }
    caret
}

fn previous_boundary(text: &str, caret: usize) -> Option<usize> {
    text[..caret].chars().next_back().map(|c| caret - c.len_utf8())
}

fn next_boundary(text: &str, caret: usize) -> Option<usize> {
    text[caret..].chars().next().map(|c| caret + c.len_utf8())
}

fn prepare_edit(state: &mut ComposerState) -> bool {
    if state.select_all {
        state.text.clear();
        state.select_all = false;
        state.caret = 0;
        return true;
    }
    false
}

fn normalize_single_line(text: &str) -> String {
    let mut normalized = String::with_capacity(text.len().min(PASTE_LIMIT_BYTES));
    let mut previous_was_space = false;
    for character in text.chars() {
        if normalized.len().saturating_add(character.len_utf8()) > PASTE_LIMIT_BYTES {
            break;
        }
        let character = match character {
            '\r' | '\n' | '\t' => ' ',
            value if value.is_control() => continue,
            value => value,
        };
        if character == ' ' && previous_was_space {
            continue;
        }
        previous_was_space = character == ' ';
        normalized.push(character);
    }
    normalized
}

#[cfg(test)]
mod tests {
    use super::*;

    fn state(text: &str) -> ComposerState {
        ComposerState {
            text: text.to_owned(),
            caret: text.len(),
            ..ComposerState::default()
        }
    }

    // --- the window -------------------------------------------------------

    /// The defect this exists for: a line that outgrows its box must keep the
    /// caret's neighbourhood visible rather than painting from the head.
    #[test]
    fn a_line_wider_than_the_box_shows_the_caret_not_its_beginning() {
        let text = "0123456789abcdefghij";
        let window = visible_window(text, "", text.len(), 1, 10);
        assert!(window.truncated);
        assert_eq!(&text[window.text..], "cdefghij");
    }

    /// A caret moved to the start must drag the view with it. Anchoring to the
    /// end was correct only while the caret could not move; leaving it there
    /// would mean editing text that is off screen.
    #[test]
    fn the_window_follows_the_caret_backwards() {
        let text = "0123456789abcdefghij";
        let window = visible_window(text, "", 0, 1, 10);
        assert_eq!(window.text, 0, "a caret at the start shows the start");
        assert!(!window.truncated);

        let middle = visible_window(text, "", 10, 1, 10);
        assert!(
            middle.text <= 10,
            "the window must contain the caret, not start after it"
        );
    }

    #[test]
    fn a_line_that_fits_is_shown_whole_and_unmarked() {
        let window = visible_window("echo ok", "", 7, 1, 40);
        assert_eq!(window, VisibleWindow::default());
    }

    /// The box is measured in cells, not characters: a CJK line that fits by
    /// character count would overflow by half its width again.
    #[test]
    fn double_width_characters_are_charged_two_cells() {
        let text = "中文中文中";
        assert_eq!(cells(text), 10);
        let window = visible_window(text, "", text.len(), 1, 8);
        assert!(window.truncated);
        assert_eq!(cells(&text[window.text..]), 6);
        assert!(text.is_char_boundary(window.text));
    }

    #[test]
    fn a_preedit_filling_the_box_hides_the_committed_text_completely() {
        let text = "committed";
        let preedit = "中文中文中文";
        let window = visible_window(text, preedit, text.len(), 1, 6);
        assert_eq!(window.text, text.len());
        assert!(window.preedit > 0);
        assert!(preedit.is_char_boundary(window.preedit));
    }

    #[test]
    fn a_box_with_no_room_yields_valid_empty_offsets() {
        let text = "text";
        let preedit = "pre";
        for width in 0..3 {
            for caret in [0, 2, text.len()] {
                let window = visible_window(text, preedit, caret, 1, width);
                assert!(text.is_char_boundary(window.text));
                assert!(preedit.is_char_boundary(window.preedit));
            }
        }
    }

    /// The visible span must never exceed the box, or the painter clips it
    /// again and the caret leaves the box exactly as before.
    #[test]
    fn the_visible_span_always_fits_the_box_it_was_measured_for() {
        let text = "a".repeat(500);
        let preedit = "中".repeat(20);
        for width in 3..40 {
            for caret in [0, 1, 250, text.len()] {
                let window = visible_window(&text, &preedit, caret, 1, width);
                let caret = caret.max(window.text);
                let painted = usize::from(window.truncated)
                    + cells(&text[window.text..caret])
                    + cells(&preedit[window.preedit..])
                    + 1;
                assert!(
                    painted <= width,
                    "width {width} caret {caret} painted {painted} cells"
                );
            }
        }
    }

    // --- the caret --------------------------------------------------------

    /// The whole point of the caret: text can be inserted in the middle, not
    /// only appended.
    #[test]
    fn typing_inserts_at_the_caret_rather_than_at_the_end() {
        let mut composer = state("echo  ok");
        composer.caret = 5;
        insert(&mut composer, "HERE");
        assert_eq!(composer.text, "echo HERE ok");
        assert_eq!(composer.caret, 9, "the caret follows what was typed");
    }

    #[test]
    fn backspace_deletes_before_the_caret_and_delete_deletes_at_it() {
        let mut composer = state("abcd");
        composer.caret = 2;
        backspace(&mut composer);
        assert_eq!(composer.text, "acd");
        assert_eq!(composer.caret, 1);

        delete_forward(&mut composer);
        assert_eq!(composer.text, "ad");
        assert_eq!(composer.caret, 1, "delete leaves the caret where it was");
    }

    #[test]
    fn deleting_at_either_edge_is_a_no_op_rather_than_a_panic() {
        let mut composer = state("ab");
        composer.caret = 0;
        backspace(&mut composer);
        assert_eq!(composer.text, "ab");
        composer.caret = 2;
        delete_forward(&mut composer);
        assert_eq!(composer.text, "ab");
    }

    /// Byte offsets into UTF-8: a caret that steps by one byte would land
    /// inside a character and every later slice would panic.
    #[test]
    fn the_caret_moves_by_characters_not_bytes() {
        let mut composer = state("中a文");
        move_caret(&mut composer, Move::LineStart);
        assert_eq!(composer.caret, 0);
        move_caret(&mut composer, Move::Right);
        assert_eq!(composer.caret, 3, "past the whole wide character");
        move_caret(&mut composer, Move::Right);
        assert_eq!(composer.caret, 4);
        move_caret(&mut composer, Move::Left);
        assert_eq!(composer.caret, 3);
        move_caret(&mut composer, Move::LineEnd);
        assert_eq!(composer.caret, composer.text.len());
        // Moving past either end stops rather than wrapping or overflowing.
        move_caret(&mut composer, Move::Right);
        assert_eq!(composer.caret, composer.text.len());
        move_caret(&mut composer, Move::LineStart);
        move_caret(&mut composer, Move::Left);
        assert_eq!(composer.caret, 0);
    }

    /// The caret can outlive the text it pointed into: the accessibility bus
    /// can replace the contents underneath it. Clamping is what keeps a stale
    /// offset from panicking on the next slice.
    #[test]
    fn a_stale_caret_is_clamped_to_a_character_boundary() {
        let mut composer = state("中");
        composer.caret = 999;
        insert(&mut composer, "x");
        assert_eq!(composer.text, "中x");

        // An offset inside a character clamps *down*, to the boundary before
        // it. Backspace then has nothing behind it, which is the safe reading:
        // rounding up would delete a character the caret was never past.
        let mut composer = state("中");
        composer.caret = 1;
        backspace(&mut composer);
        assert_eq!(composer.text, "中", "no character is deleted from a bad offset");
        assert!(composer.text.is_char_boundary(composer.caret));

        let mut composer = state("中x");
        composer.caret = 2; // inside the wide character
        delete_forward(&mut composer);
        assert_eq!(composer.text, "x", "clamped down, so the wide character goes");
        assert!(composer.text.is_char_boundary(composer.caret));
    }

    // --- pointer ----------------------------------------------------------

    /// Clicking is how a caret gets anywhere without arrow keys, and the
    /// column has to resolve against the same cells the painter drew.
    #[test]
    fn a_click_lands_on_the_character_under_the_pointer() {
        let text = "abcdef";
        assert_eq!(caret_at_cell(text, 0, 0), 0);
        assert_eq!(caret_at_cell(text, 0, 2), 2);
        assert_eq!(
            caret_at_cell(text, 0, 99),
            text.len(),
            "past the end lands at the end"
        );
    }

    /// A wide character owns two cells; clicking its right half puts the caret
    /// after it, which is where the pointer visually is.
    #[test]
    fn a_click_inside_a_wide_character_resolves_to_its_nearer_edge() {
        let text = "中x";
        assert_eq!(caret_at_cell(text, 0, 0), 0, "left half stays before it");
        assert_eq!(caret_at_cell(text, 0, 1), 3, "right half lands after it");
        assert_eq!(caret_at_cell(text, 0, 2), 3);
    }

    /// A scrolled line resolves clicks against the visible window, or the
    /// caret lands on a character the user cannot see.
    #[test]
    fn a_click_is_measured_from_the_first_visible_character() {
        let text = "0123456789";
        assert_eq!(caret_at_cell(text, 4, 0), 4);
        assert_eq!(caret_at_cell(text, 4, 3), 7);
    }

    #[test]
    fn every_click_offset_is_a_character_boundary() {
        let text = "a中b文c";
        for cell in 0..12 {
            let caret = caret_at_cell(text, 0, cell);
            assert!(text.is_char_boundary(caret), "cell {cell} -> {caret}");
        }
    }

    // --- selection and limits --------------------------------------------

    #[test]
    fn selected_input_replaces_the_whole_buffer() {
        let mut composer = state("old");
        select_all(&mut composer);
        insert(&mut composer, "new");
        assert_eq!(composer.text, "new");
        assert!(!composer.select_all);
        assert_eq!(composer.caret, 3);
    }

    #[test]
    fn all_insert_paths_bound_total_bytes_at_a_utf8_boundary() {
        let mut composer = state(&"a".repeat(COMPOSER_LIMIT_BYTES - 2));
        insert(&mut composer, "中文");
        assert_eq!(composer.text.len(), COMPOSER_LIMIT_BYTES - 2);

        insert(&mut composer, "bcdef");
        assert_eq!(composer.text.len(), COMPOSER_LIMIT_BYTES);
        assert!(composer.text.ends_with("bc"));

        paste(&mut composer, "ignored");
        assert_eq!(composer.text.len(), COMPOSER_LIMIT_BYTES);

        select_all(&mut composer);
        insert(&mut composer, &"中".repeat(COMPOSER_LIMIT_BYTES));
        assert!(composer.text.len() <= COMPOSER_LIMIT_BYTES);
        assert_eq!(composer.text.len() % '中'.len_utf8(), 0);
    }

    #[test]
    fn copy_and_cut_require_a_selection() {
        let mut composer = state("copy me");
        assert_eq!(selected_text(&composer.text, &composer.select_all), None);
        select_all(&mut composer);
        assert_eq!(
            selected_text(&composer.text, &composer.select_all),
            Some("copy me")
        );
        assert_eq!(cut(&mut composer), Some("copy me".to_owned()));
        assert!(composer.text.is_empty());
        assert_eq!(composer.caret, 0);
    }

    #[test]
    fn paste_is_single_line_bounded_and_filters_controls() {
        let mut composer = state("replace");
        select_all(&mut composer);
        paste(&mut composer, "a\r\nb\t\u{1b}c");
        assert_eq!(composer.text, "a b c");
        assert!(!composer.select_all);
    }

    #[test]
    fn replace_text_is_single_line_and_puts_the_caret_at_the_end() {
        let mut composer = state("old");
        select_all(&mut composer);
        replace_text(&mut composer, "cu33o\nmarker");
        assert_eq!(composer.text, "cu33o marker");
        assert!(!composer.select_all);
        assert_eq!(composer.caret, composer.text.len());
    }

    #[test]
    fn cancel_and_submit_clear_transient_state_atomically() {
        let mut composer = ComposerState {
            text: "echo ok".to_owned(),
            preedit: "中".to_owned(),
            focused: true,
            select_all: true,
            submit_error: Some("old failure".to_owned()),
            caret: 7,
        };
        assert_eq!(composer.take_submission().as_deref(), Some("echo ok\r"));
        assert_eq!(composer.text, "");
        assert_eq!(composer.preedit, "");
        assert!(!composer.select_all);
        assert_eq!(composer.submit_error, None);
        assert!(composer.focused);

        composer.restore_failed_submission("retry\r".to_owned(), "PTY is closed".to_owned());
        assert_eq!(composer.text, "retry");
        assert_eq!(composer.submit_error.as_deref(), Some("PTY is closed"));
        assert!(composer.focused);

        composer.preedit = "文".to_owned();
        composer.select_all = true;
        composer.cancel_focus();
        assert!(!composer.focused);
        assert_eq!(composer.preedit, "");
        assert!(!composer.select_all);
        assert_eq!(composer.submit_error, None);
    }
}
