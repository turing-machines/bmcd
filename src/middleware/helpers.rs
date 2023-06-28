/// small helper macro which handles the code duplication of declaring gpio lines.
#[macro_export]
macro_rules! setup_output_array {
    // this macro has 2 match patterns, the differenciator is the 'arr' prefix.
    // which is used to switch from creating a output object with arr of values
    // to an array of output objects containing one value.
    ($chip:ident, arr [$($pin:ident),+], $active: expr) => {
       [
           $(
               setup_output_array!($chip, [$pin], $active)
           ),*
       ]
    };

    ($chip:ident, $output:expr, $active: expr) => {
       $chip
        .request_lines(gpiod::Options::output($output).active($active))
        .context(concat!("error initializing pin ", stringify!($pin)))?
    };

}
