use artisan_broker::{
    ProcessEnvironment, evaluate, policy_from_environment, signals_from_environment,
};

fn main() {
    match run() {
        Ok(block) => println!("{block}"),
        Err(error) => {
            eprintln!("Artisan Broker configuration failed: {error}");
            std::process::exit(2);
        }
    }
}

fn run() -> Result<bool, Box<dyn std::error::Error>> {
    let environment = ProcessEnvironment;
    let policy = policy_from_environment(&environment)?;
    let signals = signals_from_environment(&environment)?;
    Ok(evaluate(&policy, &signals).block)
}
