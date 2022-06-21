#![feature(result_option_inspect)]
use std::io::{Write, self, Read, BufReader, BufRead};
use zf::other_error;

const HELP_STR: &'static str = r#"Usage: zf [options]

    -f, --filter     Skip interactive use and filter using the given query
    -k, --keep-order Don't sort by rank and preserve order of lines read on stdin
    -l, --lines      Set the maximum number of result lines to show (default 10)
    -p, --plain      Disable filename match prioritization
    -v, --version    Show version information and exit
    -h, --help       Display this help and exit"#;

const VERSION_STR: &'static str = "0.5-dev";


#[derive(Debug, Clone, Eq, PartialEq)]
pub struct Config {
    pub help: bool,
    pub version: bool,
    pub skip_ui: bool,
    pub keep_order: bool,
    pub lines: usize,
    pub plain: bool,
    pub query: String,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            help: false,
            version: false,
            skip_ui: false,
            keep_order: false,
            lines: 10,
            plain: false,
            query: "".into(),
        }
    }
}

pub type AnyError = Box<dyn std::error::Error + 'static>;

impl Config {
    pub fn parse(args: &[String]) -> Result<Self, AnyError> {
        let mut config = Config::default();

        let mut skip = false;
        for idx in 1..args.len() {
            if skip { skip = false; continue; }

            match &*args[idx] {
                "-h" | "--help" => {
                    config.help = true;
                    break;
                },
                "-v" | "--version" => {
                    config.version = true;
                    break;
                },
                "-k" | "--keep-order" => {
                    config.keep_order = true;
                },
                "-p" | "--plain" => {
                    config.plain = true;
                },
                "-l" | "--lines" => {
                    if idx + 1 < args.len() {
                        config.lines = args[idx+1].parse()?;
                        skip = true;
                        if config.lines == 0 {
                            return Err(Box::new(other_error("InvalidCharacter")));
                        }
                    } else {
                        return Err(Box::new(other_error(format!("option '{}' requires an argument\n{}", args[idx], HELP_STR))));
                    }
                },
                "-f" | "--filter" => {
                    config.skip_ui = true;
                    if idx + 1 < args.len() {
                        config.query = args[idx+1].clone();
                        skip = true;
                    } else {
                        return Err(Box::new(other_error(format!("option '{}' requires an argument\n{}", args[idx], HELP_STR))));
                    }
                },
                _ => {
                    return Err(Box::new(other_error(format!(
                        "unrecognized option '{}'\n{}", args[idx], HELP_STR
                    ))));
                }
            }
        }

        Ok(config)
    }
}

fn main() -> Result<(), AnyError>{
    let mut stdout = std::io::stdout();
    let mut stderr = std::io::stderr();

    let args = Vec::from_iter(std::env::args());
    let config = Config::parse(&args).inspect_err(|e| eprintln!("{}", e))?;
    println!("{:?}", config);

    if config.help {
        write!(stdout, "{}", HELP_STR)?;
    } else if config.version {
        write!(stdout, "{}", VERSION_STR)?;
    } else {
        let candidates = zf::Candidate::collect(BufReader::new(std::io::stdin()), b'\n', config.plain);
        if candidates.len() > 0 {
            if config.skip_ui {
                for candidate in zf::rank_candidates(candidates, &config.query, config.keep_order) {
                    println!("{}", candidate.path);
                }
            } else {
                if let Ok(select)  = {
                    let mut terminal = zf::Terminal::new(candidates.len().min(config.lines))?;
                    let r = terminal.run(candidates, config.keep_order);
                    // terminal.clean_up()?;
                    r
                } {
                    println!("{}", select);
                } else {
                    println!("fail to get select");
                }

                // if let Ok(select) = terminal.run(candidates, config.keep_order) {
                //     // println!("{}", select.path);
                // } else {
                //     std::process::exit(1);
                // }
            }
        }

    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn check_eq_config(args: &[&'static str], config: Config) {
        let out = Config::parse(&Vec::from_iter(args.into_iter().map(|&s| s.to_owned()))).expect(
            "Expect an Ok(...) not Err(...)"
        );
        assert_eq!(out, config);
    }

    #[test]
    fn parse_sample() {
        for (args, out) in vec![
            (vec!["zf"], Config::default()),
            (vec!["zf", "--help"], Config { help: true ,..Config::default()}),
            (vec!["zf", "--version"], Config { version: true ,..Config::default()}),
            (vec!["zf", "-v", "-h"], Config { version: true, help: false,..Config::default()}),
            (vec!["zf", "-f", "query"], Config { skip_ui: true, query: "query".into(), help: false,..Config::default()}),
            (vec!["zf", "-l", "12"], Config { lines: 12, help: false,..Config::default()}),
            (vec!["zf", "-k", "-p"], Config { keep_order: true, plain: true,..Config::default()}),
            (vec!["zf", "--keep-order", "--plain"], Config { keep_order: true, plain: true,..Config::default()}),
        ].into_iter() {
            check_eq_config(&args, out);
        }

        for args in vec![
            (vec!["zf", "--filter"]),
            (vec!["zf", "asdf"]),
            (vec!["zf", "bad arg here", "--help"]),
            (vec!["zf", "--lines", "-10"]),
        ].into_iter() {
            assert!(Config::parse(&Vec::from_iter(args.into_iter().map(|s| s.to_owned()))).is_err());
        }
    }
}
