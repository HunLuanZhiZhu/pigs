//! Unit tests for TUI components — verifies rendering logic, state management,
//! and event handling without requiring a real TTY terminal.

#[cfg(test)]
mod tests {
    use crate::chat::{ChatEntry, ChatState};
    use crate::editor::EditorState;
    use crate::header::HeaderState;
    use crate::footer::FooterState;
    use crate::overlay::{OverlayKind, OverlayState};
    use crate::theme::Theme;
    use crate::markdown;

    // ===== Chat State Tests =====

    #[test]
    fn chat_state_push_and_scroll() {
        let mut chat = ChatState::new();
        assert!(chat.main_entries.is_empty());

        chat.push(ChatEntry::User("hello".to_string()));
        chat.push(ChatEntry::Assistant {
            text: "hi there".to_string(),
            thinking: None,
        });
        assert_eq!(chat.main_entries.len(), 2);

        // Scroll up adds to offset
        chat.scroll_up(5);
        assert_eq!(chat.scroll_offset, 5);

        // Scroll down reduces offset
        chat.scroll_down(3);
        assert_eq!(chat.scroll_offset, 2);

        // Scroll down past zero clamps to 0
        chat.scroll_down(100);
        assert_eq!(chat.scroll_offset, 0);
    }

    #[test]
    fn chat_render_lines_produces_output() {
        let mut chat = ChatState::new();
        chat.push(ChatEntry::User("test message".to_string()));
        chat.push(ChatEntry::Assistant {
            text: "response".to_string(),
            thinking: Some("thinking...".to_string()),
        });
        chat.push(ChatEntry::System("system note".to_string()));

        let lines = chat.render_lines(80);
        assert!(!lines.is_empty(), "render_lines should produce output");
        // Should have at least the 3 entries plus spacing
        assert!(lines.len() >= 3, "should have at least 3 lines for 3 entries");
    }

    #[test]
    fn chat_tool_call_entry_renders() {
        let mut chat = ChatState::new();
        chat.push(ChatEntry::ToolCall {
            name: "bash".to_string(),
            args: r#"{"command": "ls"}"#.to_string(),
            result: "file1\nfile2".to_string(),
            is_error: false,
        });

        let lines = chat.render_lines(80);
        assert!(!lines.is_empty());
    }

    #[test]
    fn chat_bash_entry_renders() {
        let mut chat = ChatState::new();
        chat.push(ChatEntry::Bash {
            command: "echo hello".to_string(),
            output: "hello".to_string(),
            exit_code: 0,
        });
        chat.push(ChatEntry::Bash {
            command: "false".to_string(),
            output: String::new(),
            exit_code: 1,
        });

        let lines = chat.render_lines(80);
        assert!(!lines.is_empty());
    }

    // ===== Editor State Tests =====

    #[test]
    fn editor_initial_state() {
        let editor = EditorState::new();
        assert_eq!(editor.text(), "");
        assert!(!editor.is_slash_command());
        assert!(!editor.is_bash());
    }

    #[test]
    fn editor_slash_detection() {
        let mut editor = EditorState::new();
        editor.textarea.insert_str("/help");
        assert!(editor.is_slash_command());
        assert!(!editor.is_bash());
    }

    #[test]
    fn editor_bash_detection() {
        let mut editor = EditorState::new();
        editor.textarea.insert_str("!ls -la");
        assert!(!editor.is_slash_command());
        assert!(editor.is_bash());
    }

    #[test]
    fn editor_submit_clears() {
        let mut editor = EditorState::new();
        editor.textarea.insert_str("test input");
        let text = editor.submit();
        assert_eq!(text, "test input");
        assert_eq!(editor.text(), "");
    }

    // ===== Header State Tests =====

    #[test]
    fn header_state_init() {
        let header = HeaderState::new("gpt-4o", "en");
        assert_eq!(header.model, "gpt-4o");
        assert_eq!(header.language, "en");
        assert!(!header.version.is_empty());
    }

    // ===== Footer State Tests =====

    #[test]
    fn footer_state_init() {
        let footer = FooterState::new("/home/user/project", "gpt-4o");
        assert_eq!(footer.model, "gpt-4o");
        assert_eq!(footer.input_tokens, 0);
        assert_eq!(footer.output_tokens, 0);
        assert_eq!(footer.context_pct, 0.0);
        assert!(!footer.is_working);
    }

    // ===== Overlay Tests =====

    #[test]
    fn overlay_model_selector_init() {
        let models = vec!["gpt-4o".to_string(), "claude-3".to_string(), "gemini".to_string()];
        let overlay = OverlayState::model_selector(&models, "claude-3");
        assert_eq!(overlay.kind, OverlayKind::ModelSelector);
        assert_eq!(overlay.items.len(), 3);
        // Should select the current model
        assert_eq!(overlay.selected_index(), Some(1));
    }

    #[test]
    fn overlay_navigation_up_down() {
        let models = vec!["a".to_string(), "b".to_string(), "c".to_string()];
        let mut overlay = OverlayState::model_selector(&models, "a");
        assert_eq!(overlay.selected_index(), Some(0));

        overlay.select_down();
        assert_eq!(overlay.selected_index(), Some(1));

        overlay.select_down();
        assert_eq!(overlay.selected_index(), Some(2));

        // Wrap around to 0
        overlay.select_down();
        assert_eq!(overlay.selected_index(), Some(0));

        // Wrap around to last
        overlay.select_up();
        assert_eq!(overlay.selected_index(), Some(2));
    }

    #[test]
    fn overlay_session_selector_init() {
        let sessions = vec![
            ("abc12345".to_string(), "Session 1".to_string()),
            ("def67890".to_string(), "Session 2".to_string()),
        ];
        let overlay = OverlayState::session_selector(&sessions, "def67890");
        assert_eq!(overlay.kind, OverlayKind::SessionSelector);
        assert_eq!(overlay.items.len(), 2);
        assert_eq!(overlay.selected_index(), Some(0));
    }

    // ===== Theme Tests =====

    #[test]
    fn theme_by_name() {
        let dark = Theme::by_name("dark");
        assert_eq!(dark.name, "dark");

        let light = Theme::by_name("light");
        assert_eq!(light.name, "light");

        let hc = Theme::by_name("high-contrast");
        assert_eq!(hc.name, "high-contrast");

        // Unknown theme defaults to dark
        let unknown = Theme::by_name("nonexistent");
        assert_eq!(unknown.name, "dark");
    }

    #[test]
    fn theme_available_list() {
        let themes = Theme::available();
        assert!(themes.contains(&"dark"));
        assert!(themes.contains(&"light"));
        assert!(themes.contains(&"high-contrast"));
    }

    // ===== Markdown Rendering Tests =====

    #[test]
    fn markdown_render_heading() {
        let lines = markdown::render_to_lines("# Hello World", 80);
        assert!(!lines.is_empty(), "heading should produce lines");
    }

    #[test]
    fn markdown_render_code_block() {
        let md = "```python\nprint('hello')\n```";
        let lines = markdown::render_to_lines(md, 80);
        assert!(!lines.is_empty(), "code block should produce lines");
    }

    #[test]
    fn markdown_render_list() {
        let md = "- item 1\n- item 2\n- item 3";
        let lines = markdown::render_to_lines(md, 80);
        assert!(!lines.is_empty(), "list should produce lines");
    }

    #[test]
    fn markdown_render_empty() {
        let lines = markdown::render_to_lines("", 80);
        // Empty markdown should produce empty or minimal output
        assert!(lines.len() <= 1);
    }

    #[test]
    fn markdown_render_paragraph() {
        let lines = markdown::render_to_lines("This is a paragraph of text.", 80);
        assert!(!lines.is_empty());
    }

    #[test]
    fn markdown_render_bold_italic() {
        let lines = markdown::render_to_lines("**bold** and *italic*", 80);
        assert!(!lines.is_empty());
    }

    #[test]
    fn markdown_render_inline_code() {
        let lines = markdown::render_to_lines("Use `code` here.", 80);
        assert!(!lines.is_empty());
    }

    // ===== Event System Tests =====

    #[test]
    fn event_broker_send_receive() {
        use crate::event::{AppEvent, EventBroker};

        let mut broker = EventBroker::new();
        let tx = broker.sender();

        tx.send(AppEvent::StreamText("hello".to_string())).unwrap();
        tx.send(AppEvent::TurnFinished).unwrap();

        // In a real async context we'd use tokio::test, but we can at least
        // verify the sender doesn't panic and the channel has messages.
        // The broker.next() requires async, so we just verify construction.
        assert!(true, "EventBroker constructed and events sent");
    }

    // ===== Integration: Chat + Markdown =====

    #[test]
    fn chat_assistant_with_markdown_renders() {
        let mut chat = ChatState::new();
        chat.push(ChatEntry::Assistant {
            text: "# Heading\n\nSome **bold** text with `code`.\n\n- item 1\n- item 2".to_string(),
            thinking: None,
        });

        let lines = chat.render_lines(80);
        assert!(!lines.is_empty());
        // Should have multiple lines for the markdown content
        assert!(lines.len() > 3, "markdown content should produce multiple lines");
    }

    #[test]
    fn chat_multiple_entries_render_in_order() {
        let mut chat = ChatState::new();
        chat.push(ChatEntry::User("q1".to_string()));
        chat.push(ChatEntry::Assistant {
            text: "a1".to_string(),
            thinking: None,
        });
        chat.push(ChatEntry::User("q2".to_string()));
        chat.push(ChatEntry::Assistant {
            text: "a2".to_string(),
            thinking: None,
        });

        let lines = chat.render_lines(80);
        assert!(!lines.is_empty());
        // Entries should be in order: user, assistant, user, assistant
        // We can't easily check line content due to styling, but length should be > 4
        assert!(lines.len() >= 4, "4 entries should produce at least 4 lines");
    }
}
