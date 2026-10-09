fn main() {
    let path = std::env::args().nth(1).expect("usage: tally FILE");
    let text = std::fs::read_to_string(path).expect("readable file");
    println!("{}", text.split_whitespace().count());
}
