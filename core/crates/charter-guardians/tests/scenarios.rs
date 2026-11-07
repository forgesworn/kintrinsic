//! scenarios.json: every scenario, every step — outcome and projected state.
mod common;

#[test]
fn every_scenario_step_matches() {
    let v = common::load("scenarios.json");
    let scenarios = v["scenarios"].as_array().unwrap();
    let mut steps = 0;
    for sc in scenarios {
        common::replay(sc);
        steps += sc["steps"].as_array().unwrap().len();
    }
    assert!(!scenarios.is_empty());
    eprintln!("{} scenarios, {steps} steps passed", scenarios.len());
}
