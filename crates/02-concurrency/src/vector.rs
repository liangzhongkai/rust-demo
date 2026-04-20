use anyhow::{Result, anyhow};
use std::ops::{Add, AddAssign, Mul};

pub struct Vector<T> {
    data: Vec<T>,
}

pub fn dot_product<T>(a: Vector<T>, b: Vector<T>) -> Result<T>
where
    T: Copy + Default + Add<Output = T> + AddAssign + Mul<Output = T>,
{
    Ok(T::default())
}
