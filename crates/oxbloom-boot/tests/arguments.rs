use oxbloom_boot::ApplicationArguments;

fn args(a: &[&str]) -> ApplicationArguments {
    ApplicationArguments::new(a.iter().copied())
}

#[test]
fn key_value_option() {
    let a = args(&["--port=8080"]);
    assert_eq!(a.option("port"), Some("8080"));
    assert_eq!(a.option("missing"), None);
}

#[test]
fn flag_option_has_empty_value() {
    assert_eq!(args(&["--verbose"]).option("verbose"), Some(""));
}

#[test]
fn value_keeps_later_equals_signs() {
    assert_eq!(args(&["--k=a=b"]).option("k"), Some("a=b"));
}

#[test]
fn repeated_option_keeps_all_values_in_order() {
    let a = args(&["--k=1", "--x=9", "--k=2", "--k"]);
    assert_eq!(a.option("k"), Some("1"));
    assert_eq!(a.options("k"), ["1", "2", ""]);
    assert!(a.options("none").is_empty());
}

#[test]
fn double_dash_ends_options() {
    let a = args(&["--a", "--", "--b"]);
    assert_eq!(a.option("a"), Some(""));
    assert_eq!(a.option("b"), None);
    assert_eq!(a.non_option_args(), ["--b"]);
}

#[test]
fn empty_name_and_single_dash_are_non_options() {
    let a = args(&["--=x", "-x", "word"]);
    assert_eq!(a.non_option_args(), ["--=x", "-x", "word"]);
    assert_eq!(a.option_names().count(), 0);
}

#[test]
fn option_names_unique_in_first_seen_order() {
    let a = args(&["--b=1", "--a=2", "--b=3", "--A"]);
    assert_eq!(a.option_names().collect::<Vec<_>>(), ["b", "a", "A"]);
}

#[test]
fn raw_keeps_every_argument_in_order() {
    let a = args(&["--a=1", "x", "--", "--b"]);
    assert_eq!(a.raw(), ["--a=1", "x", "--", "--b"]);
}

#[test]
fn debug_shows_counts_not_values() {
    let a = args(&["--token=s3cret", "s3cret"]);
    let d = format!("{a:?}");
    assert!(!d.contains("s3cret"));
    assert_eq!(d, "ApplicationArguments { options: 1, non_option_args: 1 }");
}

#[test]
fn empty_input() {
    let a = ApplicationArguments::new(Vec::<String>::new());
    assert!(a.raw().is_empty());
    assert!(a.non_option_args().is_empty());
    assert_eq!(a, ApplicationArguments::default());
}

#[test]
fn second_double_dash_is_a_non_option() {
    let a = args(&["--", "x", "--"]);
    assert_eq!(a.non_option_args(), ["x", "--"]);
    assert_eq!(a.option_names().count(), 0);
}

#[test]
fn double_dash_as_last_argument() {
    let a = args(&["--a=1", "--"]);
    assert_eq!(a.option("a"), Some("1"));
    assert!(a.non_option_args().is_empty());
    assert_eq!(a.raw(), ["--a=1", "--"]);
}

#[test]
fn key_value_after_double_dash_is_a_non_option() {
    let a = args(&["--", "--k=v"]);
    assert_eq!(a.option("k"), None);
    assert_eq!(a.option_names().count(), 0);
    assert_eq!(a.non_option_args(), ["--k=v"]);
}

#[test]
fn empty_string_is_a_non_option() {
    let a = args(&[""]);
    assert_eq!(a.non_option_args(), [""]);
    assert_eq!(a.option_names().count(), 0);
}

#[test]
fn unicode_names_and_values() {
    let a = args(&["--ключ=значение"]);
    assert_eq!(a.option("ключ"), Some("значение"));
}

#[test]
fn trailing_equals_gives_empty_value() {
    assert_eq!(args(&["--k="]).option("k"), Some(""));
}

#[test]
fn different_values_are_not_equal() {
    assert_ne!(args(&["--a=1"]), args(&["--a=2"]));
}

#[test]
fn display_shows_counts_only() {
    let args = ApplicationArguments::new(["--a=secret", "--a=x", "--b", "serve", "--", "--c=y"]);
    assert_eq!(
        args.to_string(),
        "ApplicationArguments(2 options, 2 non-option arguments)"
    );
    assert!(!args.to_string().contains("secret"));
}
