use std::fmt;

/// Parsed command-line arguments of an Oxbloom application.
///
/// `--k=v` is an option (the value is everything after the first `=`), `--k`
/// is an option with the empty value, `--` ends option parsing, and everything
/// else (`-x`, `--=x`, plain words, anything after `--`) is a non-option
/// argument. Names are case-sensitive.
///
/// `Debug` and `Display` show counts only, never names or values.
///
/// ```
/// use oxbloom_boot::ApplicationArguments;
///
/// let args = ApplicationArguments::new(["--name=oxbloom", "--", "--rest"]);
/// assert_eq!(args.option("name"), Some("oxbloom"));
/// assert_eq!(args.non_option_args(), ["--rest"]);
/// ```
#[derive(Clone, Default, PartialEq, Eq)]
#[non_exhaustive]
pub struct ApplicationArguments {
    raw: Vec<String>,
    options: Vec<(String, Vec<String>)>,
    non_option_args: Vec<String>,
}

impl ApplicationArguments {
    /// Parses `args`, which should not include the program name.
    ///
    /// ```
    /// use oxbloom_boot::ApplicationArguments;
    ///
    /// let args = ApplicationArguments::new(["--k=a=b", "-x"]);
    /// assert_eq!(args.option("k"), Some("a=b"));
    /// assert_eq!(args.non_option_args(), ["-x"]);
    /// ```
    pub fn new(args: impl IntoIterator<Item = impl Into<String>>) -> Self {
        let raw: Vec<String> = args.into_iter().map(Into::into).collect();
        let mut options: Vec<(String, Vec<String>)> = Vec::new();
        let mut non_option_args = Vec::new();
        let mut ended = false;
        for arg in &raw {
            if ended {
                non_option_args.push(arg.clone());
                continue;
            }
            if arg == "--" {
                ended = true;
                continue;
            }
            let parsed = arg.strip_prefix("--").and_then(|rest| {
                let (name, value) = rest.split_once('=').unwrap_or((rest, ""));
                (!name.is_empty()).then_some((name, value))
            });
            match parsed {
                Some((name, value)) => match options.iter_mut().find(|(n, _)| n == name) {
                    Some((_, values)) => values.push(value.to_owned()),
                    None => options.push((name.to_owned(), vec![value.to_owned()])),
                },
                None => non_option_args.push(arg.clone()),
            }
        }
        Self {
            raw,
            options,
            non_option_args,
        }
    }

    /// First value of option `name`, if present.
    ///
    /// ```
    /// use oxbloom_boot::ApplicationArguments;
    ///
    /// let args = ApplicationArguments::new(["--k=1", "--k=2"]);
    /// assert_eq!(args.option("k"), Some("1"));
    /// assert_eq!(args.option("other"), None);
    /// ```
    pub fn option(&self, name: &str) -> Option<&str> {
        self.options(name).first().map(String::as_str)
    }

    /// All values of option `name` in order; empty when absent.
    ///
    /// ```
    /// use oxbloom_boot::ApplicationArguments;
    ///
    /// let args = ApplicationArguments::new(["--k=1", "--k=2"]);
    /// assert_eq!(args.options("k"), ["1", "2"]);
    /// assert!(args.options("absent").is_empty());
    /// ```
    pub fn options(&self, name: &str) -> &[String] {
        self.options
            .iter()
            .find(|(n, _)| n == name)
            .map_or(&[], |(_, v)| v.as_slice())
    }

    /// Option names, each once, in first-seen order.
    ///
    /// ```
    /// use oxbloom_boot::ApplicationArguments;
    ///
    /// let args = ApplicationArguments::new(["--b=1", "--a=2", "--b=3"]);
    /// assert_eq!(args.option_names().collect::<Vec<_>>(), ["b", "a"]);
    /// ```
    pub fn option_names(&self) -> impl Iterator<Item = &str> {
        self.options.iter().map(|(n, _)| n.as_str())
    }

    /// Arguments that are not options, in order.
    ///
    /// ```
    /// use oxbloom_boot::ApplicationArguments;
    ///
    /// let args = ApplicationArguments::new(["run", "--a", "-x"]);
    /// assert_eq!(args.non_option_args(), ["run", "-x"]);
    /// ```
    pub fn non_option_args(&self) -> &[String] {
        &self.non_option_args
    }

    /// Every argument exactly as given, in order.
    ///
    /// ```
    /// use oxbloom_boot::ApplicationArguments;
    ///
    /// let args = ApplicationArguments::new(["--a=1", "x"]);
    /// assert_eq!(args.raw(), ["--a=1", "x"]);
    /// ```
    pub fn raw(&self) -> &[String] {
        &self.raw
    }
}

impl fmt::Debug for ApplicationArguments {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "ApplicationArguments {{ options: {}, non_option_args: {} }}",
            self.options.len(),
            self.non_option_args.len()
        )
    }
}

impl fmt::Display for ApplicationArguments {
    /// Shows counts only: the number of distinct option names and of
    /// non-option arguments. Names and values may contain secrets.
    ///
    /// ```
    /// use oxbloom_boot::ApplicationArguments;
    ///
    /// let args = ApplicationArguments::new(["--api.key=s3cr3t", "serve"]);
    /// assert_eq!(
    ///     args.to_string(),
    ///     "ApplicationArguments(1 options, 1 non-option arguments)"
    /// );
    /// ```
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "ApplicationArguments({} options, {} non-option arguments)",
            self.options.len(),
            self.non_option_args.len()
        )
    }
}
