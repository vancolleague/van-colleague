use std::mem::discriminant;

use serde::{Deserialize, Serialize};

struct CLICommandSynonym {
    cli_command: CLICommand,
    text: &'static str,
}

const CLI_COMMANDS: [CLICommandSynonym; 4] = [
    CLICommandSynonym {
        cli_command: CLICommand::Start,
        text: "start",
    },
    CLICommandSynonym {
        cli_command: CLICommand::Stop,
        text: "stop",
    },
    CLICommandSynonym {
        cli_command: CLICommand::Status,
        text: "status",
    },
    CLICommandSynonym {
        cli_command: CLICommand::Restart,
        text: "restart",
    },
];

#[derive(Debug, Copy, Clone, PartialEq, Eq, Hash, Deserialize, Serialize)]
pub enum CLICommand {
    Start,
    Stop,
    Status,
    Restart,
}

impl CLICommand {
    fn same_variant(&self, other: &Self) -> bool {
        discriminant(self) == discriminant(other)
    }

    pub fn from_str(s: &str) -> Result<Self, &'static str> {
        let input = s.to_lowercase();

        for command in CLI_COMMANDS {
            if command.text == input {
                return Ok(command.cli_command);
            }
        }

        Err("Bad Action name given")
    }

    pub fn to_str(&self) -> &'static str {
        for command in CLI_COMMANDS {
            if self.same_variant(&command.cli_command) {
                return command.text;
            }
        }
        ""
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cli_command_synonyms_count() {
        use std::mem;
        assert_eq!(mem::variant_count::<CLICommand>(), CLI_COMMANDS.len());
    }

    #[test]
    fn cli_command_same_variant() {
        use CLICommand as C;

        let one = C::Start;
        let two = C::Start;
        assert!(one.same_variant(&two));

        let three = C::Start;
        let four = C::Restart;
        assert!(!three.same_variant(&four));
    }

    #[test]
    fn cli_command_from_str() {
        use CLICommand as C;

        let text = "start";
        let cli_command = C::from_str(text);
        assert_eq!(cli_command, Ok(C::Start));

        let text = "Start";
        let cli_command = C::from_str(text);
        assert_eq!(cli_command, Ok(C::Start));
    }

    #[test]
    fn cli_command_to_str() {
        let text = "start";
        assert_eq!(text, CLICommand::Start.to_str());
    }
}
