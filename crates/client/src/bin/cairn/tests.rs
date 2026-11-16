use super::{TokenInput, token_input};

#[test]
fn token_input_falls_back_when_the_terminal_cannot_hide() {
    assert_eq!(token_input(false, || unreachable!()), TokenInput::Piped);
    assert_eq!(token_input(true, || true), TokenInput::Hidden);
    assert_eq!(token_input(true, || false), TokenInput::Visible);
}
