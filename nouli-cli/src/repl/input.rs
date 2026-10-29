//! What a line typed at the prompt means, before anything is done with it.

/// A line, classified. Questions and fact text are kept as typed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum Line {
    /// An empty line: send the queue again when a failed turn left its question.
    Send,
    Fact(String),
    /// The text after `?`, in the question grammar.
    Question(String),
    Command(Command),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum Command {
    Help,
    Use(String),
    New(String),
    Branch(String),
    Blend {
        sources: Vec<String>,
        name: String,
    },
    /// The question a what-if asks, without its `?`.
    Whatif(String),
    Replace(usize, String),
    Facts,
    View,
    Context,
    Clear,
    Quit,
    /// A command that needs an argument it did not get; carries the usage line.
    Usage(&'static str),
    Unknown(String),
}

pub(super) fn parse(line: &str) -> Line {
    let trimmed = line.trim();
    if trimmed.is_empty() {
        return Line::Send;
    }
    if let Some(question) = trimmed.strip_prefix('?') {
        return Line::Question(question.trim().to_owned());
    }
    let Some(command) = trimmed.strip_prefix('/') else {
        return Line::Fact(trimmed.to_owned());
    };
    let (name, argument) = command
        .split_once(char::is_whitespace)
        .map_or((command, ""), |(name, rest)| (name, rest.trim()));
    Line::Command(match name {
        "help" => Command::Help,
        "facts" => Command::Facts,
        "view" => Command::View,
        "context" => Command::Context,
        "clear" => Command::Clear,
        "quit" | "exit" => Command::Quit,
        "use" => single(argument, Command::Use, "/use SESSION"),
        "new" => single(argument, Command::New, "/new NAME"),
        "branch" => single(argument, Command::Branch, "/branch NEW_NAME"),
        "whatif" => match argument.strip_prefix('?') {
            Some(question) => Command::Whatif(question.trim().to_owned()),
            None => Command::Usage("/whatif ?id: prompt [a, b]"),
        },
        "replace" => replace(argument),
        "blend" => blend(argument),
        other => Command::Unknown(other.to_owned()),
    })
}

fn single(argument: &str, make: fn(String) -> Command, usage: &'static str) -> Command {
    if argument.is_empty() || argument.contains(char::is_whitespace) {
        Command::Usage(usage)
    } else {
        make(argument.to_owned())
    }
}

fn replace(argument: &str) -> Command {
    const USAGE: &str = "/replace N TEXT, where N is a number from /facts";
    argument
        .split_once(char::is_whitespace)
        .and_then(|(number, text)| {
            let number = number.parse::<usize>().ok().filter(|number| *number > 0)?;
            let text = text.trim();
            (!text.is_empty()).then(|| Command::Replace(number, text.to_owned()))
        })
        .unwrap_or(Command::Usage(USAGE))
}

fn blend(argument: &str) -> Command {
    const USAGE: &str = "/blend A B [...] as NEW_NAME";
    let words: Vec<_> = argument.split_whitespace().collect();
    match words.as_slice() {
        [sources @ .., "as", name] if sources.len() >= 2 => Command::Blend {
            sources: sources.iter().map(|source| (*source).to_owned()).collect(),
            name: (*name).to_owned(),
        },
        _ => Command::Usage(USAGE),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plain_lines_are_facts_question_lines_keep_their_text_and_blank_sends() {
        assert_eq!(parse(""), Line::Send);
        assert_eq!(parse("   "), Line::Send);
        assert_eq!(
            parse("  The train is late. "),
            Line::Fact("The train is late.".into())
        );
        assert_eq!(
            parse("?transport: How? [train, taxi]"),
            Line::Question("transport: How? [train, taxi]".into())
        );
        assert_eq!(parse("?"), Line::Question(String::new()));
    }

    #[test]
    fn commands_take_exactly_the_arguments_they_need() {
        let command = |line: &str| match parse(line) {
            Line::Command(command) => command,
            other => panic!("{other:?}"),
        };
        assert_eq!(command("/help"), Command::Help);
        assert_eq!(command("/use travel"), Command::Use("travel".into()));
        assert_eq!(command("/use"), Command::Usage("/use SESSION"));
        assert_eq!(command("/use a b"), Command::Usage("/use SESSION"));
        assert_eq!(
            command("/whatif ?late: Will it be late?"),
            Command::Whatif("late: Will it be late?".into())
        );
        for bad in ["/whatif", "/whatif The fare doubled."] {
            assert!(matches!(command(bad), Command::Usage(_)), "{bad}");
        }
        assert_eq!(
            command("/replace 3 The train is late."),
            Command::Replace(3, "The train is late.".into())
        );
        for bad in [
            "/replace",
            "/replace 3",
            "/replace x text",
            "/replace 0 text",
        ] {
            assert!(matches!(command(bad), Command::Usage(_)), "{bad}");
        }
        assert_eq!(
            command("/blend travel budget as plan"),
            Command::Blend {
                sources: vec!["travel".into(), "budget".into()],
                name: "plan".into()
            }
        );
        for bad in ["/blend travel as plan", "/blend a b", "/blend"] {
            assert!(matches!(command(bad), Command::Usage(_)), "{bad}");
        }
        assert_eq!(command("/exit"), Command::Quit);
        assert_eq!(command("/nonsense x"), Command::Unknown("nonsense".into()));
    }
}
