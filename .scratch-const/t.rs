pub struct Wrap<const N: usize>;
pub trait Twice { const VALUE: usize; }
impl<const N: usize> Twice for Wrap<N> { const VALUE: usize = N * 2; }
pub fn f<const N: usize>() -> [u8; <Wrap<N> as Twice>::VALUE] { [0u8; <Wrap<N> as Twice>::VALUE] }
pub const fn g<const N: usize>() -> usize { <Wrap<N> as Twice>::VALUE }
