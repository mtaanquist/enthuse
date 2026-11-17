fn main() {
    // Our own components read from ui/theme.slint; the few standard widgets
    // we keep (ListView, LineEdit) use the dark Fluent style to match.
    let config = slint_build::CompilerConfiguration::new().with_style("fluent-dark".into());
    slint_build::compile_with_config("ui/app.slint", config).expect("compile Slint UI");
}
