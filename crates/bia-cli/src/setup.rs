//! `bia setup`: first-run setup from a terminal, for installs without the UI (ADR-0062).
//!
//! On a terminal it prompts, with the password hidden and asked twice. Otherwise it reads the
//! code and then the password from one line each of stdin, so a script can pipe them in;
//! neither is ever taken from argv, which other users can read.

use std::io::{BufRead, IsTerminal, Write};

use anyhow::{Context, bail};
use bia_client::Client;

pub(crate) async fn run(client: &Client) -> anyhow::Result<()> {
    let (code, password) = if std::io::stdin().is_terminal() {
        prompt()?
    } else {
        read_lines(&mut std::io::stdin().lock())?
    };
    let done = client.set_up(&code, &password).await?;
    eprintln!(
        "Setup complete. This admin Token ({}) is shown only now; send it as BIA_TOKEN:",
        done.token.token.id
    );
    println!("{}", done.token.value);
    Ok(())
}

fn prompt() -> anyhow::Result<(String, String)> {
    eprint!("Setup code (or the whole Setup Link) from the `bia serve` logs: ");
    std::io::stderr().flush()?;
    let mut code = String::new();
    std::io::stdin().read_line(&mut code)?;
    let password = rpassword::prompt_password("Password (at least 12 characters): ")?;
    if rpassword::prompt_password("Password again: ")? != password {
        bail!("the passwords differ; nothing was set");
    }
    Ok((code_of(&code), password))
}

fn read_lines(input: &mut impl BufRead) -> anyhow::Result<(String, String)> {
    let mut lines = input.lines();
    let mut next = |what: &str| {
        lines
            .next()
            .transpose()?
            .with_context(|| format!("stdin ended before the {what}"))
    };
    let code = next("setup code")?;
    let password = next("password")?;
    Ok((code_of(&code), password))
}

/// The code alone, from either the code or the whole Setup Link.
fn code_of(input: &str) -> String {
    let input = input.trim();
    input
        .split_once("code=")
        .map_or(input, |(_, code)| code.split('&').next().unwrap_or(code))
        .to_owned()
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use super::{code_of, read_lines};

    #[test]
    fn the_code_comes_alone_or_inside_the_setup_link() {
        assert_eq!(code_of(" 1a2b-3c4d\n"), "1a2b-3c4d");
        assert_eq!(
            code_of("http://localhost:8080/setup?code=1a2b-3c4d"),
            "1a2b-3c4d"
        );
    }

    #[test]
    fn piped_input_is_the_code_then_the_password_and_keeps_its_spaces() {
        let (code, password) = read_lines(&mut &b"1a2b-3c4d\n correct horse \n"[..]).unwrap();
        assert_eq!(code, "1a2b-3c4d");
        assert_eq!(password, " correct horse ");

        let error = read_lines(&mut &b"1a2b-3c4d\n"[..]).unwrap_err();
        assert!(error.to_string().contains("password"), "{error}");
    }
}
