/// small helper macro which handles the code duplication of declaring gpio lines.
#[macro_export]
macro_rules! gpio_output_lines {
    ($chip:ident, $direction:expr, $output:expr) => {
        $chip
            .request_lines(gpiod::Options::output($output).active($direction))
            .context(concat!("error initializing pin ", stringify!($output)))?
    };
}

/// uses [gpio_output_lines] to declare an array of gpiod::Lines objects
#[macro_export]
macro_rules! gpio_output_array {
    ($chip:ident, $direction:expr, $($pin:ident),+) => {
       [
           $(
               $crate::gpio_output_lines!($chip, $direction, [$pin])
           ),*
       ]
    };
}
