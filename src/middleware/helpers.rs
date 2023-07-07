/// small helper macro which handles the code duplication of declaring gpio lines.
#[macro_export]
macro_rules! setup_output_array {
    // this macro has 2 match patterns, the differenciator is the 'arr' prefix.
    // which is used to switch from creating a output object with arr of values
    // to an array of output objects containing one value.
    ($chip:ident, $($pin:ident),+) => {
       [
           $(
               setup_output_array!($chip, [$pin])
           ),*
       ]
    };

    ($chip:ident, $output:expr) => {
       $chip
        .request_lines(gpiod::Options::output($output))
        .context(concat!("error initializing pin ", stringify!($output)))?
    };

}
