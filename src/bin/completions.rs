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

fn complete_spin_subcommand(name: &str, input: impl CompletionInput) -> Vec<String> {
    let command = SpinApp::command();
    let Some(subcommand) = command.find_subcommand(name) else {
        return vec![];  // TODO: is there a way to hand off to a plugin?
    };

    if subcommand.has_subcommands() {
        // TODO: make this properly recursive instead of hardwiring to 2 levels of subcommand
        if input.arg_index() <= 2 {
            let sub_subcommands = subcommand.get_subcommands().filter(|c| !c.is_hide_set()).map(|c| c.get_name());
            return input.complete_subcommand(sub_subcommands);
        } else {
            let ssc = input.args()[2];
            let Some(sub_subcommand) = subcommand.find_subcommand(ssc) else {
                return vec![];
            };
            return complete_cmd(sub_subcommand, 2, input);
        }
    }

    return complete_cmd(subcommand, 1, input);
}

fn complete_cmd(cmd: &clap::Command, depth: usize, input: impl CompletionInput) -> Vec<String> {
    // Strategy:
    // If the PREVIOUS word was a PARAMETERISED option:
    // - Figure out possible values and offer them
    // Otherwise (i.e. if the PREVIOUS word was a NON-OPTION (did not start with '-'), or a UNARY option):
    // - If ALL positional parameters are satisfied:
    //   - Offer the options
    // - Otherwise:
    //   - If the current word is EMPTY and the NEXT available positional is completable:
    //     - Offer the NEXT positional
    //   - If the current word is EMPTY and the NEXT positional is NON-COMPLETABLE:
    //     - Offer the options
    //   - If the current word is NON-EMTPY:
    //     - Offer the options AND the NEXT positional if completable

    let prev_arg = cmd.get_arguments().find(|o| o.is_match(input.previous_word()));

    // Are we in a position of completing a value-ful flag?
    if let Some(prev_option) = prev_arg {
        if prev_option.is_takes_value_set() {
            // TODO: possible completions
            return input.complete_subcommand(["bish", "bash", "honk"]);
        }
    }

    // No: previous word was not a flag, or was unary (or was unknown)

    // Are all positional parameters satisfied?
    let num_positionals = cmd.get_positionals().count();
    let first_unfulfilled_positional = if num_positionals == 0 {
        None
    } else {
        // // This *includes* the arg in progress. The arg may have been completed! E.g.
        // // spin up -f spin.toml|   => 2
        // // spin up -f spin.toml |  => 3
        // let num_args_provided = input.arg_index() - depth;
        let mut num_positionals_provided = 0;
        let in_progress = !(input.args().last().unwrap().is_empty());  // safe to unwrap because we are deep in subcommanery here
        let mut provided = input.args().into_iter().skip(depth + 1);
        let mut prev: Option<&str> = None;
        let mut last_was_positional = false;
        loop {
            let Some(cur) = provided.next() else {
                if in_progress && last_was_positional {
                    num_positionals_provided -= 1;
                }
                break;
            };

            if cur.is_empty() {
                continue;
            }

            let is_cur_positional = if cur.starts_with('-') {
                false
            } else {
                // It might be a positional or it might be governed by a flag
                let is_governed_by_prev = match prev {
                    None => false,
                    Some(p) => {
                        let matching_opt = cmd.get_arguments().find(|a| a.long_and_short().contains(&p.to_string()));
                        match matching_opt {
                            None => false,  // the previous thing was not an option, so cannot govern
                            Some(o) => o.is_takes_value_set(),
                        }
                    },
                };
                !is_governed_by_prev
            };

            if is_cur_positional {
                // eprintln!("Found a pos!  '{cur}'");
                num_positionals_provided += 1;
            }

            // if !cur.starts_with('-') {  // if it's a flag, it can't be a positional
            //     if let Some(p) = prev {
            //         if p.starts_with('-') {  // was it preceded by a flag?
            //             if let Some(matching_opt) = cmd.get_arguments().find(|a| a.long_and_short().contains(&p.to_string())) {
            //                 if !matching_opt.is_takes_value_set() {  // did that flag govern this value?
            //                     num_positionals_provided += 1;  // No! It's a positional!
            //                 }
            //             }
            //         }
            //     }
            // }

            last_was_positional = is_cur_positional;
            prev = Some(cur);

        }
        // eprintln!("NPProv = {num_positionals_provided} (of {num_positionals} declared on command)");
        cmd.get_positionals().nth(num_positionals_provided)
    };

    match first_unfulfilled_positional {
        Some(arg) => {
            let cands = ["pish", "posh", "tosh"].iter().map(|s| format!("{s}{num_positionals}{}", arg.get_name())).collect::<Vec<_>>();
            return input.complete_subcommand(cands.iter().map(|s| s.as_str()));
        },
        None => {
            // TODO: consider positionals
            let all_args: Vec<_> = cmd.get_arguments().flat_map(|o| o.long_and_short()).collect();
            return input.complete_subcommand(all_args.iter().map(|s| s.as_str()));
        },
    }
}

trait ArgInfo {
    fn long_and_short(&self) -> Vec<String>;

    fn is_match(&self, text: &str) -> bool {
        self.long_and_short().contains(&text.to_string())
    }
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
