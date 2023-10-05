use clap::CommandFactory;
use shell_completion::{CompletionInput, CompletionSet};
use spin_cli::SpinApp;

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

fn complete_spin_commands(input: impl CompletionInput) -> Vec<String> {
    let command = SpinApp::command();

    // --help and --version don't show up as options so this doesn't complete them,
    // but I'm not going to lose much sleep over that.

    // TODO: this doesn't currently offer plugin names as completions.

    let candidates = command.get_subcommands().filter(|c| !c.is_hide_set()).map(|c| c.get_name());
    input.complete_subcommand(candidates)
}

fn complete_spin_subcommand(_subcommand: &str, _input: impl CompletionInput) -> Vec<String> {
    vec![]
}

trait ArgInfo {
    fn long_and_short(&self) -> Vec<String>;
}

impl<'a> ArgInfo for clap::Arg<'a> {
    fn long_and_short(&self) -> Vec<String> {
        let mut result = vec![];
        if let Some(c) = self.get_short() {
            result.push(format!("-{c}"));
        }
        if let Some(s) = self.get_long() {
            result.push(format!("--{s}"));
        }
        result
    }
}
