use shell_completion::{CompletionInput, CompletionSet};

fn main() -> anyhow::Result<()> {
    let input = shell_completion::BashCompletionInput::from_env().unwrap();
    complete(input).suggest();
    Ok(())
}

fn complete(input: impl CompletionInput) -> Vec<String> {
    match input.arg_index() {
        0 => unreachable!(),
        1 => complete_spin_commands(input),
        _ => {
            let sc = input.args()[1].to_owned();
            complete_spin_subcommand(&sc, input)
        }
    }
}

fn complete_spin_commands(_input: impl CompletionInput) -> Vec<String> {
    todo!()
}

fn complete_spin_subcommand(_subcommand: &str, _input: impl CompletionInput) -> Vec<String> {
    todo!()
}
