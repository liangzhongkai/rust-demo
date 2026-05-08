use concurrency::Matrix;

fn main() {
    let a: Matrix<i32> = Matrix::new([1, 2, 3, 4, 5, 6], 2, 3);
    let b: Matrix<i32> = Matrix::new([1, 2, 3, 4, 5, 6], 3, 2);
    println!("a * b: {}", a * b);
}
