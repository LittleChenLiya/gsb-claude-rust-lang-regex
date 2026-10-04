#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Debug, Clone, PartialEq, Eq)]
    enum Token {
        Ident,
        Int,
        Whitespace,
        Eq,
        EqEq,
        String,
        Other,
    }

    #[test]
    fn test_scan_sum_12() {
        let lexer = LexerBuilder::new()
            .state("main")
            .rule(r"[a-zA-Z_][a-zA-Z0-9_]*", Action::Emit(Token::Ident))
            .rule(r"[0-9]+", Action::Emit(Token::Int))
            .rule(r"\s+", Action::Emit(Token::Whitespace))
            .done()
            .initial_state("main")
            .build()
            .unwrap();

        let tokens = lexer.scan("sum 12").unwrap();
        assert_eq!(tokens.len(), 3);
        assert_eq!(tokens[0].ty, Token::Ident);
        assert_eq!(tokens[0].start, 0);
        assert_eq!(tokens[0].end, 3);
        assert_eq!(tokens[1].ty, Token::Whitespace);
        assert_eq!(tokens[1].start, 3);
        assert_eq!(tokens[1].end, 4);
        assert_eq!(tokens[2].ty, Token::Int);
        assert_eq!(tokens[2].start, 4);
        assert_eq!(tokens[2].end, 6);
    }

    #[test]
    fn test_scan_with_string() {
        let lexer = LexerBuilder::new()
            .state("main")
            .rule(r"[a-zA-Z_][a-zA-Z0-9_]*", Action::Emit(Token::Ident))
            .rule(r"[0-9]+", Action::Emit(Token::Int))
            .rule(r"\s+", Action::Emit(Token::Whitespace))
            .rule(r#""[^"]*""#, Action::Emit(Token::String))
            .done()
            .initial_state("main")
            .build()
            .unwrap();

        let tokens = lexer.scan(r#"x "hello" 42"#).unwrap();
        assert_eq!(tokens.len(), 5);
        assert_eq!(tokens[0].ty, Token::Ident);
        assert_eq!(tokens[0].start, 0);
        assert_eq!(tokens[0].end, 1);
        assert_eq!(tokens[1].ty, Token::Whitespace);
        assert_eq!(tokens[1].start, 1);
        assert_eq!(tokens[1].end, 2);
        assert_eq!(tokens[2].ty, Token::String);
        assert_eq!(tokens[2].start, 2);
        assert_eq!(tokens[2].end, 9);
        assert_eq!(tokens[3].ty, Token::Whitespace);
        assert_eq!(tokens[3].start, 9);
        assert_eq!(tokens[3].end, 10);
        assert_eq!(tokens[4].ty, Token::Int);
        assert_eq!(tokens[4].start, 10);
        assert_eq!(tokens[4].end, 12);
    }

    #[test]
    fn test_unclosed_string_error() {
        let lexer = LexerBuilder::new()
            .state("main")
            .rule(r"[a-zA-Z_][a-zA-Z0-9_]*", Action::Emit(Token::Ident))
            .rule(r"\s+", Action::Emit(Token::Whitespace))
            .rule(r#"""#, Action::Switch("string".to_string()))
            .done()
            .state("string")
            .rule(r#"""#, Action::Switch("main".to_string()))
            .rule(r#"[^"]+"#, Action::Emit(Token::String))
            .terminal(false)
            .done()
            .initial_state("main")
            .build()
            .unwrap();

        let result = lexer.scan(r#"x "hello"#);
        assert!(result.is_err());
        match result.unwrap_err() {
            LexerError::UnexpectedEnd { state } => {
                assert_eq!(state, "string");
            }
            e => panic!("expected UnexpectedEnd, got {:?}", e),
        }
    }

    #[test]
    fn test_longest_match_eq_eq() {
        let lexer = LexerBuilder::new()
            .state("main")
            .rule(r"=", Action::Emit(Token::Eq))
            .rule(r"==", Action::Emit(Token::EqEq))
            .done()
            .initial_state("main")
            .build()
            .unwrap();

        let tokens = lexer.scan("==").unwrap();
        assert_eq!(tokens.len(), 1);
        assert_eq!(tokens[0].ty, Token::EqEq);
        assert_eq!(tokens[0].start, 0);
        assert_eq!(tokens[0].end, 2);

        let tokens = lexer.scan("=").unwrap();
        assert_eq!(tokens.len(), 1);
        assert_eq!(tokens[0].ty, Token::Eq);
        assert_eq!(tokens[0].start, 0);
        assert_eq!(tokens[0].end, 1);

        let tokens = lexer.scan("===").unwrap();
        assert_eq!(tokens.len(), 2);
        assert_eq!(tokens[0].ty, Token::EqEq);
        assert_eq!(tokens[0].start, 0);
        assert_eq!(tokens[0].end, 2);
        assert_eq!(tokens[1].ty, Token::Eq);
        assert_eq!(tokens[1].start, 2);
        assert_eq!(tokens[1].end, 3);
    }

    #[test]
    fn test_same_length_declaration_order() {
        let lexer = LexerBuilder::new()
            .state("main")
            .rule(r"ab", Action::Emit(Token::Eq))
            .rule(r"ab", Action::Emit(Token::EqEq))
            .done()
            .initial_state("main")
            .build()
            .unwrap();

        let tokens = lexer.scan("ab").unwrap();
        assert_eq!(tokens.len(), 1);
        assert_eq!(tokens[0].ty, Token::Eq);
    }

    #[test]
    fn test_non_ascii() {
        let lexer = LexerBuilder::new()
            .state("main")
            .rule(r"[a-zA-Z_][a-zA-Z0-9_]*", Action::Emit(Token::Ident))
            .rule(r"[0-9]+", Action::Emit(Token::Int))
            .rule(r"\s+", Action::Emit(Token::Whitespace))
            .rule(r"[^\x00-\x7F]+", Action::Emit(Token::Other))
            .done()
            .initial_state("main")
            .build()
            .unwrap();

        let tokens = lexer.scan("café 42").unwrap();
        assert_eq!(tokens.len(), 4);
        assert_eq!(tokens[0].ty, Token::Ident);
        assert_eq!(tokens[0].start, 0);
        assert_eq!(tokens[0].end, 3);
        assert_eq!(tokens[1].ty, Token::Other);
        assert_eq!(tokens[1].start, 3);
        assert_eq!(tokens[1].end, 5);
        assert_eq!(tokens[2].ty, Token::Whitespace);
        assert_eq!(tokens[2].start, 5);
        assert_eq!(tokens[2].end, 6);
        assert_eq!(tokens[3].ty, Token::Int);
        assert_eq!(tokens[3].start, 6);
        assert_eq!(tokens[3].end, 8);
    }

    #[test]
    fn test_no_match_error() {
        let lexer = LexerBuilder::new()
            .state("main")
            .rule(r"[a-zA-Z_][a-zA-Z0-9_]*", Action::Emit(Token::Ident))
            .rule(r"[0-9]+", Action::Emit(Token::Int))
            .rule(r"\s+", Action::Emit(Token::Whitespace))
            .done()
            .initial_state("main")
            .build()
            .unwrap();

        let result = lexer.scan("sum @ 12");
        assert!(result.is_err());
        match result.unwrap_err() {
            LexerError::NoMatch { position, state } => {
                assert_eq!(position, 4);
                assert_eq!(state, "main");
            }
            e => panic!("expected NoMatch, got {:?}", e),
        }
    }

    #[test]
    fn test_empty_match_rejected() {
        let result = LexerBuilder::new()
            .state("main")
            .rule(r"[a-zA-Z_]*", Action::Emit(Token::Ident))
            .rule(r"[0-9]+", Action::Emit(Token::Int))
            .done()
            .initial_state("main")
            .build();

        assert!(result.is_err());
        match result.unwrap_err() {
            LexerError::BuildError(msg) => {
                assert!(msg.contains("empty string"));
            }
            e => panic!("expected BuildError, got {:?}", e),
        }
    }

    #[test]
    fn test_recursive_state_limit() {
        let lexer = LexerBuilder::new()
            .state("a")
            .rule(r"x", Action::Switch("b".to_string()))
            .done()
            .state("b")
            .rule(r"x", Action::Switch("a".to_string()))
            .done()
            .initial_state("a")
            .max_state_switches(5)
            .build()
            .unwrap();

        let result = lexer.scan("xxxxxx");
        assert!(result.is_err());
        match result.unwrap_err() {
            LexerError::TooManyStateSwitches { count, limit } => {
                assert_eq!(count, 6);
                assert_eq!(limit, 5);
            }
            e => panic!("expected TooManyStateSwitches, got {:?}", e),
        }
    }

    #[test]
    fn test_skip_action() {
        let lexer = LexerBuilder::new()
            .state("main")
            .rule(r"[a-zA-Z_][a-zA-Z0-9_]*", Action::Emit(Token::Ident))
            .rule(r"\s+", Action::Skip)
            .done()
            .initial_state("main")
            .build()
            .unwrap();

        let tokens = lexer.scan("  sum  ").unwrap();
        assert_eq!(tokens.len(), 1);
        assert_eq!(tokens[0].ty, Token::Ident);
        assert_eq!(tokens[0].start, 2);
        assert_eq!(tokens[0].end, 5);
    }

    #[test]
    fn test_build_error_no_states() {
        let result = LexerBuilder::<Token>::new().build();
        assert!(result.is_err());
    }

    #[test]
    fn test_build_error_no_initial_state() {
        let result = LexerBuilder::new()
            .state("main")
            .rule(r"x", Action::Emit(Token::Ident))
            .done()
            .build();
        assert!(result.is_err());
    }

    #[test]
    fn test_build_error_initial_state_not_exist() {
        let result = LexerBuilder::new()
            .state("main")
            .rule(r"x", Action::Emit(Token::Ident))
            .done()
            .initial_state("nonexistent")
            .build();
        assert!(result.is_err());
    }

    #[test]
    fn test_build_error_state_no_rules() {
        let result = LexerBuilder::new()
            .state("main")
            .done()
            .initial_state("main")
            .build();
        assert!(result.is_err());
    }

    #[test]
    fn test_build_error_switch_nonexistent() {
        let result = LexerBuilder::new()
            .state("main")
            .rule(r"x", Action::Switch("nonexistent".to_string()))
            .done()
            .initial_state("main")
            .build();
        assert!(result.is_err());
    }

    #[test]
    fn test_build_error_invalid_pattern() {
        let result = LexerBuilder::new()
            .state("main")
            .rule(r"[", Action::Emit(Token::Ident))
            .done()
            .initial_state("main")
            .build();
        assert!(result.is_err());
    }

    #[test]
    fn test_empty_input() {
        let lexer = LexerBuilder::new()
            .state("main")
            .rule(r"x", Action::Emit(Token::Ident))
            .done()
            .initial_state("main")
            .build()
            .unwrap();

        let tokens = lexer.scan("").unwrap();
        assert_eq!(tokens.len(), 0);
    }

    #[test]
    fn test_state_info() {
        let lexer = LexerBuilder::new()
            .state("main")
            .rule(r"x", Action::Emit(Token::Ident))
            .done()
            .state("string")
            .rule(r#"""#, Action::Emit(Token::String))
            .done()
            .initial_state("main")
            .build()
            .unwrap();

        assert_eq!(lexer.state_len(), 2);
        assert_eq!(lexer.state_name(0), Some("main"));
        assert_eq!(lexer.state_name(1), Some("string"));
        assert_eq!(lexer.state_name(2), None);
    }
}
