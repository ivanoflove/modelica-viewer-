use std::time::Instant;

#[derive(Clone, Copy, Debug, Default, Eq, Ord, PartialEq, PartialOrd)]
pub struct TextPosition {
    pub line: usize,
    /// UTF-8 byte offset within `line`; every constructor keeps this on a char boundary.
    pub column: usize,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct TextRange {
    pub start: TextPosition,
    pub end: TextPosition,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct EditImpact {
    pub start_line: usize,
    pub old_line_count: usize,
    pub new_line_count: usize,
    pub affects_folding: bool,
    pub full_reset: bool,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TextBuffer {
    lines: Vec<String>,
    /// The line ending after each line, when one exists. Mixed source files are preserved.
    endings: Vec<String>,
    preferred_ending: String,
}

impl TextBuffer {
    pub fn new(source: &str) -> Self {
        let (lines, endings) = split_lines(source);
        let mut counts = std::collections::HashMap::<&str, usize>::new();
        for ending in &endings {
            *counts.entry(ending).or_default() += 1;
        }
        let preferred_ending = endings
            .iter()
            .max_by_key(|ending| counts.get(ending.as_str()).copied().unwrap_or_default())
            .cloned()
            .unwrap_or_else(|| "\n".to_owned());
        Self {
            lines,
            endings,
            preferred_ending,
        }
    }

    pub fn line_count(&self) -> usize {
        self.lines.len()
    }

    pub fn lines(&self) -> &[String] {
        &self.lines
    }

    pub fn line(&self, index: usize) -> &str {
        self.lines.get(index).map_or("", String::as_str)
    }

    pub fn max_line_chars(&self) -> usize {
        self.lines
            .iter()
            .map(|line| line.chars().count())
            .max()
            .unwrap_or_default()
    }

    pub fn text(&self) -> String {
        let mut result = String::new();
        for (index, line) in self.lines.iter().enumerate() {
            result.push_str(line);
            if let Some(ending) = self.endings.get(index) {
                result.push_str(ending);
            }
        }
        result
    }

    pub fn clamp_position(&self, mut position: TextPosition) -> TextPosition {
        position.line = position.line.min(self.lines.len().saturating_sub(1));
        let line = self.line(position.line);
        position.column = position.column.min(line.len());
        while !line.is_char_boundary(position.column) {
            position.column = position.column.saturating_sub(1);
        }
        position
    }

    pub fn char_column(&self, position: TextPosition) -> usize {
        let position = self.clamp_position(position);
        self.line(position.line)[..position.column].chars().count()
    }

    pub fn position_at_char_column(&self, line: usize, column: usize) -> TextPosition {
        let line = line.min(self.lines.len().saturating_sub(1));
        let text = self.line(line);
        let byte_column = text
            .char_indices()
            .nth(column)
            .map_or(text.len(), |(byte, _)| byte);
        TextPosition {
            line,
            column: byte_column,
        }
    }

    pub fn line_start(&self, line: usize) -> TextPosition {
        TextPosition {
            line: line.min(self.lines.len().saturating_sub(1)),
            column: 0,
        }
    }

    pub fn line_end(&self, line: usize) -> TextPosition {
        let line = line.min(self.lines.len().saturating_sub(1));
        TextPosition {
            line,
            column: self.line(line).len(),
        }
    }

    pub fn previous_char(&self, position: TextPosition) -> TextPosition {
        let position = self.clamp_position(position);
        if position.column > 0 {
            let column = self.line(position.line)[..position.column]
                .char_indices()
                .next_back()
                .map_or(0, |(byte, _)| byte);
            TextPosition {
                line: position.line,
                column,
            }
        } else if position.line > 0 {
            self.line_end(position.line - 1)
        } else {
            position
        }
    }

    pub fn next_char(&self, position: TextPosition) -> TextPosition {
        let position = self.clamp_position(position);
        let line = self.line(position.line);
        if let Some((byte, character)) = line[position.column..].char_indices().next() {
            TextPosition {
                line: position.line,
                column: position.column + byte + character.len_utf8(),
            }
        } else if position.line + 1 < self.lines.len() {
            self.line_start(position.line + 1)
        } else {
            position
        }
    }

    pub fn selected_range(
        &self,
        cursor: TextPosition,
        anchor: Option<TextPosition>,
    ) -> Option<TextRange> {
        let anchor = self.clamp_position(anchor?);
        let cursor = self.clamp_position(cursor);
        if anchor == cursor {
            return None;
        }
        let (start, end) = if anchor < cursor {
            (anchor, cursor)
        } else {
            (cursor, anchor)
        };
        Some(TextRange { start, end })
    }

    pub fn text_in_range(&self, range: TextRange) -> String {
        let range = TextRange {
            start: self.clamp_position(range.start),
            end: self.clamp_position(range.end),
        };
        if range.start.line == range.end.line {
            return self.line(range.start.line)[range.start.column..range.end.column].to_owned();
        }
        let mut result = self.line(range.start.line)[range.start.column..].to_owned();
        for line in range.start.line..range.end.line {
            result.push_str(self.endings.get(line).map_or("\n", String::as_str));
            if line + 1 < range.end.line {
                result.push_str(self.line(line + 1));
            }
        }
        result.push_str(&self.line(range.end.line)[..range.end.column]);
        result
    }

    /// Replaces a range and returns the caret position immediately after inserted text.
    pub fn replace_range(&mut self, range: TextRange, inserted: &str) -> TextPosition {
        let start = self.clamp_position(range.start);
        let end = self.clamp_position(range.end);
        let (start, end) = if start <= end {
            (start, end)
        } else {
            (end, start)
        };
        let prefix = self.line(start.line)[..start.column].to_owned();
        let suffix = self.line(end.line)[end.column..].to_owned();
        let trailing_ending = self.endings.get(end.line).cloned();
        let (inserted_lines, inserted_endings) = split_lines(inserted);
        let last_inserted_line = inserted_lines.len().saturating_sub(1);
        let mut replacement = Vec::with_capacity(inserted_lines.len());
        for (index, line) in inserted_lines.iter().enumerate() {
            let mut value = line.clone();
            if index == 0 {
                value.insert_str(0, &prefix);
            }
            if index == last_inserted_line {
                value.push_str(&suffix);
            }
            replacement.push(value);
        }
        let inserted_end = TextPosition {
            line: start.line + last_inserted_line,
            column: if last_inserted_line == 0 {
                prefix.len() + inserted_lines[0].len()
            } else {
                inserted_lines[last_inserted_line].len()
            },
        };

        self.lines.splice(start.line..=end.line, replacement);
        let ending_end = (end.line + 1).min(self.endings.len());
        let mut replacement_endings = inserted_endings;
        if let Some(ending) = trailing_ending {
            replacement_endings.push(ending);
        }
        self.endings
            .splice(start.line..ending_end, replacement_endings);
        if self.lines.is_empty() {
            self.lines.push(String::new());
        }
        inserted_end
    }

    fn line_ending_for_insert(&self, line: usize) -> &str {
        self.endings
            .get(line)
            .or_else(|| {
                line.checked_sub(1)
                    .and_then(|previous| self.endings.get(previous))
            })
            .map_or(&self.preferred_ending, String::as_str)
    }
}

fn split_lines(source: &str) -> (Vec<String>, Vec<String>) {
    let mut lines = Vec::new();
    let mut endings = Vec::new();
    let bytes = source.as_bytes();
    let mut start = 0;
    let mut index = 0;
    while index < bytes.len() {
        let ending_len = match bytes[index] {
            b'\n' => 1,
            b'\r' if bytes.get(index + 1) == Some(&b'\n') => 2,
            b'\r' => 1,
            _ => {
                index += 1;
                continue;
            }
        };
        lines.push(source[start..index].to_owned());
        endings.push(source[index..index + ending_len].to_owned());
        index += ending_len;
        start = index;
    }
    lines.push(source[start..].to_owned());
    (lines, endings)
}

#[derive(Clone, Debug)]
struct EditOperation {
    start: TextPosition,
    removed_end: TextPosition,
    inserted_end: TextPosition,
    removed: String,
    inserted: String,
    before_cursor: TextPosition,
    before_anchor: Option<TextPosition>,
    after_cursor: TextPosition,
    after_anchor: Option<TextPosition>,
    before_state_id: u64,
    after_state_id: u64,
}

#[derive(Clone, Debug)]
pub struct SourceEditorState {
    pub document_id: Option<String>,
    pub class_name: Option<String>,
    pub source_version: u64,
    pub revision: u64,
    pub fold_revision: u64,
    pub cursor: TextPosition,
    pub selection_anchor: Option<TextPosition>,
    pub preferred_column: Option<usize>,
    pub focused: bool,
    pub dragging_selection: bool,
    pub checked_revision: Option<u64>,
    pub buffer: TextBuffer,
    current_state_id: u64,
    saved_state_id: u64,
    next_state_id: u64,
    undo: Vec<EditOperation>,
    redo: Vec<EditOperation>,
    pub last_edit_at: Option<Instant>,
    pub diagnostic: Option<String>,
    pub pending_impact: Option<EditImpact>,
    pub pending_input_at: Option<Instant>,
}

impl Default for SourceEditorState {
    fn default() -> Self {
        Self::new(None, None, 0, "")
    }
}

impl SourceEditorState {
    pub fn new(
        document_id: Option<String>,
        class_name: Option<String>,
        version: u64,
        text: &str,
    ) -> Self {
        Self {
            document_id,
            class_name,
            source_version: version,
            revision: 0,
            fold_revision: 0,
            cursor: TextPosition::default(),
            selection_anchor: None,
            preferred_column: None,
            focused: false,
            dragging_selection: false,
            checked_revision: None,
            buffer: TextBuffer::new(text),
            current_state_id: 0,
            saved_state_id: 0,
            next_state_id: 1,
            undo: Vec::new(),
            redo: Vec::new(),
            last_edit_at: None,
            diagnostic: None,
            pending_impact: None,
            pending_input_at: None,
        }
    }

    pub fn is_dirty(&self) -> bool {
        self.current_state_id != self.saved_state_id
    }

    #[cfg(test)]
    pub fn undo_len(&self) -> usize {
        self.undo.len()
    }

    pub fn mark_saved(&mut self, version: u64) {
        self.saved_state_id = self.current_state_id;
        self.source_version = version;
        self.diagnostic = None;
    }

    pub fn take_edit_impact(&mut self) -> Option<EditImpact> {
        self.pending_impact.take()
    }

    pub fn reset(
        &mut self,
        document_id: Option<String>,
        class_name: Option<String>,
        version: u64,
        text: &str,
    ) {
        *self = Self::new(document_id, class_name, version, text);
    }

    pub fn selected_text(&self) -> Option<String> {
        self.buffer
            .selected_range(self.cursor, self.selection_anchor)
            .map(|range| self.buffer.text_in_range(range))
    }

    pub fn select_all(&mut self) {
        self.selection_anchor = Some(self.buffer.line_start(0));
        self.cursor = self
            .buffer
            .line_end(self.buffer.line_count().saturating_sub(1));
        self.preferred_column = None;
    }

    pub fn move_horizontal(&mut self, direction: isize, extend: bool) {
        let selection = self
            .buffer
            .selected_range(self.cursor, self.selection_anchor);
        let next = if direction < 0 {
            selection.map_or_else(
                || self.buffer.previous_char(self.cursor),
                |range| range.start,
            )
        } else {
            selection.map_or_else(|| self.buffer.next_char(self.cursor), |range| range.end)
        };
        self.move_cursor(next, extend);
        self.preferred_column = None;
    }

    pub fn move_vertical(&mut self, delta: isize, extend: bool) {
        let preferred = self
            .preferred_column
            .unwrap_or_else(|| self.buffer.char_column(self.cursor));
        let line = (self.cursor.line as isize + delta)
            .clamp(0, self.buffer.line_count().saturating_sub(1) as isize)
            as usize;
        let next = self.buffer.position_at_char_column(line, preferred);
        self.move_cursor(next, extend);
        self.preferred_column = Some(preferred);
    }

    pub fn move_line_edge(&mut self, end: bool, extend: bool) {
        let next = if end {
            self.buffer.line_end(self.cursor.line)
        } else {
            self.buffer.line_start(self.cursor.line)
        };
        self.move_cursor(next, extend);
        self.preferred_column = None;
    }

    pub fn set_cursor(&mut self, position: TextPosition, extend: bool) {
        self.move_cursor(position, extend);
        self.preferred_column = None;
    }

    fn move_cursor(&mut self, position: TextPosition, extend: bool) {
        let position = self.buffer.clamp_position(position);
        if extend {
            self.selection_anchor.get_or_insert(self.cursor);
        } else {
            self.selection_anchor = None;
        }
        self.cursor = position;
    }

    pub fn insert_text(&mut self, text: &str) -> bool {
        let range = self
            .buffer
            .selected_range(self.cursor, self.selection_anchor)
            .unwrap_or(TextRange {
                start: self.cursor,
                end: self.cursor,
            });
        self.apply_edit(range, text)
    }

    pub fn backspace(&mut self) -> bool {
        if let Some(range) = self
            .buffer
            .selected_range(self.cursor, self.selection_anchor)
        {
            return self.apply_edit(range, "");
        }
        let previous = self.buffer.previous_char(self.cursor);
        if previous == self.cursor {
            return false;
        }
        self.apply_edit(
            TextRange {
                start: previous,
                end: self.cursor,
            },
            "",
        )
    }

    pub fn delete_forward(&mut self) -> bool {
        if let Some(range) = self
            .buffer
            .selected_range(self.cursor, self.selection_anchor)
        {
            return self.apply_edit(range, "");
        }
        let next = self.buffer.next_char(self.cursor);
        if next == self.cursor {
            return false;
        }
        self.apply_edit(
            TextRange {
                start: self.cursor,
                end: next,
            },
            "",
        )
    }

    pub fn insert_newline(&mut self) -> bool {
        let ending = self
            .buffer
            .line_ending_for_insert(self.cursor.line)
            .to_owned();
        self.insert_text(&ending)
    }

    pub fn indent(&mut self, outdent: bool) -> bool {
        let selected = self
            .buffer
            .selected_range(self.cursor, self.selection_anchor);
        if selected.is_none() {
            if outdent {
                return self.outdent_current_line();
            }
            return self.insert_text("    ");
        }
        let selected = selected.expect("checked above");
        let first_line = selected.start.line;
        let mut last_line = selected.end.line;
        if last_line > first_line && selected.end.column == 0 {
            last_line -= 1;
        }
        let range = TextRange {
            start: self.buffer.line_start(first_line),
            end: self.buffer.line_end(last_line),
        };
        let old = self.buffer.text_in_range(range);
        let (mut lines, endings) = split_lines(&old);
        if outdent {
            for line in &mut lines {
                if line.starts_with('\t') {
                    line.remove(0);
                } else {
                    let spaces = line.chars().take_while(|ch| *ch == ' ').count().min(4);
                    line.drain(..spaces);
                }
            }
        } else {
            for line in &mut lines {
                line.insert_str(0, "    ");
            }
        }
        let mut replacement = String::new();
        for (index, line) in lines.iter().enumerate() {
            replacement.push_str(line);
            if let Some(ending) = endings.get(index) {
                replacement.push_str(ending);
            }
        }
        let end = self.apply_edit(range, &replacement);
        if end {
            self.selection_anchor = Some(range.start);
        }
        end
    }

    fn outdent_current_line(&mut self) -> bool {
        let line = self.buffer.line(self.cursor.line);
        let remove = if line.starts_with('\t') {
            1
        } else {
            line.chars().take_while(|ch| *ch == ' ').count().min(4)
        };
        if remove == 0 {
            return false;
        }
        let end = self
            .buffer
            .position_at_char_column(self.cursor.line, remove);
        self.apply_edit(
            TextRange {
                start: self.buffer.line_start(self.cursor.line),
                end,
            },
            "",
        )
    }

    fn apply_edit(&mut self, range: TextRange, inserted: &str) -> bool {
        let start = self.buffer.clamp_position(range.start);
        let end = self.buffer.clamp_position(range.end);
        let (start, end) = if start <= end {
            (start, end)
        } else {
            (end, start)
        };
        let removed = self.buffer.text_in_range(TextRange { start, end });
        if removed == inserted {
            return false;
        }
        let old_line_count = end.line.saturating_sub(start.line) + 1;
        let new_line_count = split_lines(inserted).0.len();
        let affects_folding = removed
            .chars()
            .chain(inserted.chars())
            .any(|character| matches!(character, '\n' | '\r' | '(' | ')' | '{' | '}' | '[' | ']'));
        let inserted_end = self
            .buffer
            .replace_range(TextRange { start, end }, inserted);
        let impact = EditImpact {
            start_line: start.line,
            old_line_count,
            new_line_count,
            affects_folding,
            full_reset: false,
        };
        self.pending_impact = Some(match self.pending_impact {
            Some(previous) => EditImpact {
                start_line: 0,
                old_line_count: usize::MAX,
                new_line_count: self.buffer.line_count(),
                affects_folding: previous.affects_folding || impact.affects_folding,
                full_reset: true,
            },
            None => impact,
        });
        self.revision = self.revision.saturating_add(1);
        if affects_folding {
            self.fold_revision = self.fold_revision.saturating_add(1);
        }
        let before_state_id = self.current_state_id;
        let after_state_id = self.next_state_id;
        self.next_state_id = self.next_state_id.saturating_add(1);
        self.current_state_id = after_state_id;
        let after_cursor = inserted_end;
        self.undo.push(EditOperation {
            start,
            removed_end: end,
            inserted_end,
            removed,
            inserted: inserted.to_owned(),
            before_cursor: self.cursor,
            before_anchor: self.selection_anchor,
            after_cursor,
            after_anchor: None,
            before_state_id,
            after_state_id,
        });
        self.redo.clear();
        self.cursor = after_cursor;
        self.selection_anchor = None;
        self.preferred_column = None;
        self.last_edit_at = Some(Instant::now());
        self.diagnostic = None;
        self.checked_revision = None;
        true
    }

    pub fn undo(&mut self) -> bool {
        let Some(operation) = self.undo.pop() else {
            return false;
        };
        self.buffer.replace_range(
            TextRange {
                start: operation.start,
                end: operation.inserted_end,
            },
            &operation.removed,
        );
        self.cursor = self.buffer.clamp_position(operation.before_cursor);
        self.selection_anchor = operation
            .before_anchor
            .map(|position| self.buffer.clamp_position(position));
        self.current_state_id = operation.before_state_id;
        self.revision = self.revision.saturating_add(1);
        self.fold_revision = self.fold_revision.saturating_add(1);
        self.pending_impact = Some(EditImpact {
            start_line: 0,
            old_line_count: usize::MAX,
            new_line_count: self.buffer.line_count(),
            affects_folding: true,
            full_reset: true,
        });
        self.last_edit_at = Some(Instant::now());
        self.checked_revision = None;
        self.diagnostic = None;
        self.redo.push(operation);
        true
    }

    pub fn redo(&mut self) -> bool {
        let Some(operation) = self.redo.pop() else {
            return false;
        };
        let inserted_end = self.buffer.replace_range(
            TextRange {
                start: operation.start,
                end: operation.removed_end,
            },
            &operation.inserted,
        );
        self.cursor = inserted_end;
        self.selection_anchor = operation.after_anchor;
        let mut operation = operation;
        operation.inserted_end = inserted_end;
        operation.after_cursor = inserted_end;
        self.current_state_id = operation.after_state_id;
        self.undo.push(operation);
        self.revision = self.revision.saturating_add(1);
        self.fold_revision = self.fold_revision.saturating_add(1);
        self.pending_impact = Some(EditImpact {
            start_line: 0,
            old_line_count: usize::MAX,
            new_line_count: self.buffer.line_count(),
            affects_folding: true,
            full_reset: true,
        });
        self.last_edit_at = Some(Instant::now());
        self.checked_revision = None;
        self.diagnostic = None;
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn preserves_mixed_line_endings_and_trailing_newline() {
        let source = "model A\r\n  Real 值;\nend A;\r\n";
        let buffer = TextBuffer::new(source);
        assert_eq!(buffer.line_count(), 4);
        assert_eq!(buffer.text(), source);
    }

    #[test]
    fn cursor_moves_by_unicode_scalar_without_splitting_utf8() {
        let mut editor = SourceEditorState::new(None, None, 0, "A热🙂B");
        editor.cursor = editor.buffer.position_at_char_column(0, 2);
        editor.move_horizontal(1, false);
        assert_eq!(editor.buffer.char_column(editor.cursor), 3);
        editor.backspace();
        assert_eq!(editor.buffer.text(), "A热B");
        assert!(editor.buffer.line(0).is_char_boundary(editor.cursor.column));
    }

    #[test]
    fn selection_replacement_and_clipboard_text_are_utf8_safe() {
        let mut editor = SourceEditorState::new(None, None, 0, "model 热值\nend 热值;");
        editor.cursor = editor.buffer.position_at_char_column(0, 6);
        editor.selection_anchor = Some(editor.buffer.position_at_char_column(0, 6));
        editor.cursor = editor.buffer.position_at_char_column(0, 8);
        assert_eq!(editor.selected_text().as_deref(), Some("热值"));
        editor.insert_text("Temp");
        assert_eq!(editor.buffer.line(0), "model Temp");
        assert!(editor.undo());
        assert_eq!(editor.buffer.line(0), "model 热值");
        assert!(editor.redo());
        assert_eq!(editor.buffer.line(0), "model Temp");
    }

    #[test]
    fn indent_and_outdent_selected_lines_as_one_undo_step() {
        let mut editor = SourceEditorState::new(None, None, 0, "a\nb\nc");
        editor.selection_anchor = Some(TextPosition { line: 0, column: 0 });
        editor.cursor = TextPosition { line: 2, column: 1 };
        assert!(editor.indent(false));
        assert_eq!(editor.buffer.text(), "    a\n    b\n    c");
        assert_eq!(editor.undo_len(), 1);
        assert!(editor.indent(true));
        assert_eq!(editor.buffer.text(), "a\nb\nc");
    }

    #[test]
    fn multiline_edits_keep_cursor_and_line_endings_consistent() {
        let mut editor = SourceEditorState::new(None, None, 0, "A\r\nB\r\nC\r\n");
        editor.cursor = TextPosition { line: 1, column: 1 };
        editor.insert_newline();
        assert_eq!(editor.buffer.text(), "A\r\nB\r\n\r\nC\r\n");
        assert_eq!(editor.cursor, TextPosition { line: 2, column: 0 });
        assert!(editor.undo());
        assert_eq!(editor.buffer.text(), "A\r\nB\r\nC\r\n");
        assert!(editor.redo());
        assert_eq!(editor.buffer.text(), "A\r\nB\r\n\r\nC\r\n");
    }

    #[test]
    fn editor_keeps_dirty_state_and_error_text_for_further_editing() {
        let mut editor =
            SourceEditorState::new(Some("doc".into()), Some("A".into()), 4, "model A\nend A;");
        editor.insert_text("!");
        assert!(editor.is_dirty());
        editor.diagnostic = Some("语法错误".into());
        editor.insert_text("?");
        assert!(editor.is_dirty());
        assert!(editor.diagnostic.is_none());
    }

    #[test]
    fn undo_to_the_saved_edit_state_clears_dirty_without_scanning_text() {
        let mut editor = SourceEditorState::new(None, None, 2, "model Demo\nend Demo;");
        editor.insert_text(" ");
        assert!(editor.is_dirty());
        editor.mark_saved(3);
        assert!(!editor.is_dirty());
        editor.insert_text("X");
        assert!(editor.is_dirty());
        assert!(editor.undo());
        assert!(!editor.is_dirty());
        assert!(editor.redo());
        assert!(editor.is_dirty());
    }

    #[test]
    fn editor_buffers_restore_per_document_without_sharing_undo_or_caret() {
        let mut model1 = SourceEditorState::new(
            Some("doc-1".to_owned()),
            Some("Model1".to_owned()),
            0,
            "model Model1\nend Model1;",
        );
        model1.cursor = model1.buffer.line_end(0);
        model1.insert_text(" // draft");
        let saved_session = model1.clone();

        let mut model2 = SourceEditorState::new(
            Some("doc-2".to_owned()),
            Some("Model2".to_owned()),
            0,
            "model Model2\nend Model2;",
        );
        model2.cursor = model2.buffer.line_end(0);
        model2.insert_text(" // other");

        let mut restored = saved_session;
        assert_eq!(restored.document_id.as_deref(), Some("doc-1"));
        assert_eq!(restored.buffer.line(0), "model Model1 // draft");
        assert_eq!(restored.cursor, model1.cursor);
        assert!(restored.is_dirty());
        assert_eq!(restored.undo_len(), 1);
        assert_eq!(model2.buffer.line(0), "model Model2 // other");
        assert_eq!(model2.undo_len(), 1);
        assert!(restored.undo());
        assert_eq!(model2.buffer.line(0), "model Model2 // other");
    }

    #[test]
    fn ten_thousand_line_buffer_supports_local_edit_and_undo() {
        let source = (0..10_000)
            .map(|line| format!("  Real value_{line};"))
            .collect::<Vec<_>>()
            .join("\n");
        let mut editor = SourceEditorState::new(None, None, 0, &source);
        editor.cursor = TextPosition {
            line: 5_000,
            column: 7,
        };
        editor.insert_text("新");
        assert_eq!(editor.buffer.line_count(), 10_000);
        assert!(editor.buffer.line(5_000).contains("新"));
        assert!(editor.undo());
        assert_eq!(editor.buffer.text(), source);
    }
}
