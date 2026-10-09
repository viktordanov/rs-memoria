mod auth;

fn main() {
    println!("Sessions end after {} minutes.", auth::login::SESSION_MINUTES);
}
