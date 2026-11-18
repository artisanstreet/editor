pub const AGENT_NAMES: [&str; 100] = [
    "Ada",
    "Alexandra",
    "Alva",
    "Amalie",
    "Amanda",
    "Andrea",
    "Ane",
    "Angelica",
    "Anna",
    "Anne",
    "Astrid",
    "Aurora",
    "Camilla",
    "Caroline",
    "Celina",
    "Celine",
    "Charlotte",
    "Christina",
    "Dina",
    "Elisabeth",
    "Elise",
    "Ella",
    "Eline",
    "Emilie",
    "Emily",
    "Emma",
    "Erle",
    "Eva",
    "Frida",
    "Hanna",
    "Hannah",
    "Hedda",
    "Helena",
    "Helene",
    "Henriette",
    "Iben",
    "Ida",
    "Ingeborg",
    "Ingrid",
    "Isabel",
    "Isabell",
    "Isabella",
    "Iselin",
    "Jenny",
    "Johanna",
    "Johanne",
    "Josefine",
    "Julia",
    "Julie",
    "Kaja",
    "Karoline",
    "Kristina",
    "Kristine",
    "Lea",
    "Leah",
    "Lina",
    "Linnea",
    "Lisa",
    "Live",
    "Madelen",
    "Maja",
    "Malene",
    "Malin",
    "Maren",
    "Mari",
    "Maria",
    "Marie",
    "Marte",
    "Martine",
    "Mathea",
    "Mathilde",
    "Matilde",
    "Maya",
    "Mia",
    "Mille",
    "Mina",
    "Nathalie",
    "Natalie",
    "Nora",
    "Oda",
    "Pernille",
    "Ronja",
    "Sandra",
    "Sara",
    "Sarah",
    "Selma",
    "Signe",
    "Sigrid",
    "Silje",
    "Sofia",
    "Sofie",
    "Solveig",
    "Stine",
    "Sunniva",
    "Synne",
    "Thea",
    "Tiril",
    "Tuva",
    "Victoria",
    "Vilde",
];

#[must_use]
pub fn choose_agent_name<'a>(seed: &str, taken: impl IntoIterator<Item = &'a str>) -> String {
    let taken: std::collections::HashSet<String> =
        taken.into_iter().map(str::to_lowercase).collect();
    let pick = seed_hash(seed);
    let mut generation = 1u32;
    loop {
        let free: Vec<String> = AGENT_NAMES
            .iter()
            .map(|name| {
                if generation == 1 {
                    (*name).to_owned()
                } else {
                    format!("{name} {generation}")
                }
            })
            .filter(|name| !taken.contains(&name.to_lowercase()))
            .collect();
        if let Some(name) = pick_from(&free, pick) {
            return name.clone();
        }
        generation += 1;
    }
}

#[must_use]
pub fn fallback_agent_name(seed: &str) -> &'static str {
    pick_from(&AGENT_NAMES, seed_hash(seed))
        .copied()
        .unwrap_or("Agent")
}

fn pick_from<T>(from: &[T], pick: u64) -> Option<&T> {
    let len = u64::try_from(from.len()).ok().filter(|len| *len > 0)?;
    from.get(usize::try_from(pick % len).ok()?)
}

fn seed_hash(seed: &str) -> u64 {
    seed.bytes().fold(0xcbf2_9ce4_8422_2325, |hash, byte| {
        (hash ^ u64::from(byte)).wrapping_mul(0x0100_0000_01b3)
    })
}

#[cfg(test)]
#[path = "../tests/agent_names/unit.rs"]
mod tests;
