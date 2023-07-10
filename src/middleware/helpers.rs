/// small helper macro which handles the code duplication of declaring gpio lines.
#[macro_export]
macro_rules! gpio_output_lines {
    ($chip:ident, $output:expr) => {
        $chip
            .request_lines(gpiod::Options::output($output))
            .context(concat!("error initializing pin ", stringify!($output)))?
    };
}

/// uses [gpio_output_lines] to declare an array of gpiod::Lines objects
#[macro_export]
macro_rules! gpio_output_array {
    ($chip:ident, $($pin:ident),+) => {
       [
           $(
               $crate::gpio_output_lines!($chip, [$pin])
           ),*
       ]
    };
}
