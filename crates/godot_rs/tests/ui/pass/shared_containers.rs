use godot_rs::prelude::*;
#[script(base = Node)]
pub struct Scores;
#[script]
impl Scores {
    #[func]
    fn sort_scores(&self, scores: Array<i64>) -> EngineResult<()> {
        let mut local = scores.to_vec()?;
        local.sort_unstable();
        scores.assign_from_slice(&local)?;
        Ok(())
    }
}
fn main() {
    let snapshot = ArraySnapshot::from_vec(vec![1_i64, 2]);
    std::thread::spawn(move || snapshot.into_vec());
}
