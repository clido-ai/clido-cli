//! `clido refresh-models`: refresh cached model metadata.

pub fn run_update_pricing() {
    println!("Model pricing is now fetched dynamically from models.dev.");
    println!("Run 'clido refresh-models' to update the cached model metadata.");
    println!("Pricing shown in the UI comes from the models API or provider APIs.");
}
