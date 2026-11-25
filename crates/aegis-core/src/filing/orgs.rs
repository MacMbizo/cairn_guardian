//! Organisations Cairn recognises in documents (spec §7.3): a built-in list of common South
//! African and international names, plus names the owner has approved before.

/// `(display name, names to look for)`. Matching is case-insensitive on word boundaries.
pub const ORGS: &[(&str, &[&str])] = &[
    // Telecoms
    ("Vodacom", &["vodacom"]),
    ("MTN", &["mtn"]),
    ("Telkom", &["telkom"]),
    ("Cell C", &["cell c"]),
    ("Rain", &["rain (pty)", "rain mobile"]),
    ("Afrihost", &["afrihost"]),
    ("Openserve", &["openserve"]),
    // Banks and money
    ("FNB", &["fnb", "first national bank"]),
    ("ABSA", &["absa"]),
    ("Standard Bank", &["standard bank"]),
    ("Nedbank", &["nedbank"]),
    ("Capitec", &["capitec"]),
    ("Investec", &["investec"]),
    ("TymeBank", &["tymebank", "tyme bank"]),
    ("Discovery Bank", &["discovery bank"]),
    ("African Bank", &["african bank"]),
    ("PayPal", &["paypal"]),
    ("Easy Equities", &["easyequities", "easy equities"]),
    ("Allan Gray", &["allan gray"]),
    ("Coronation", &["coronation fund"]),
    ("Satrix", &["satrix"]),
    // Insurance and medical
    ("Discovery Health", &["discovery health", "discovery medical"]),
    ("Discovery", &["discovery"]),
    ("Old Mutual", &["old mutual"]),
    ("Sanlam", &["sanlam"]),
    ("Momentum", &["momentum"]),
    ("Liberty", &["liberty life", "liberty group"]),
    ("OUTsurance", &["outsurance"]),
    ("Santam", &["santam"]),
    ("Hollard", &["hollard"]),
    ("King Price", &["king price"]),
    ("Bonitas", &["bonitas"]),
    ("Medshield", &["medshield"]),
    ("Bestmed", &["bestmed"]),
    ("Fedhealth", &["fedhealth"]),
    ("GEMS", &["government employees medical scheme"]),
    ("Clicks", &["clicks"]),
    ("Dis-Chem", &["dis-chem", "dischem"]),
    // Government and utilities
    ("SARS", &["sars", "south african revenue service"]),
    ("Home Affairs", &["home affairs"]),
    ("Department of Labour", &["department of labour", "uif"]),
    ("City of Johannesburg", &["city of johannesburg"]),
    ("City of Cape Town", &["city of cape town"]),
    ("City of Tshwane", &["city of tshwane"]),
    ("eThekwini", &["ethekwini"]),
    ("Ekurhuleni", &["ekurhuleni"]),
    ("Eskom", &["eskom"]),
    ("Rand Water", &["rand water"]),
    ("RTMC", &["rtmc", "natis"]),
    ("SAPS", &["saps", "south african police"]),
    // Education
    ("UNISA", &["unisa", "university of south africa"]),
    ("Wits", &["wits", "university of the witwatersrand"]),
    ("UCT", &["uct", "university of cape town"]),
    ("University of Pretoria", &["university of pretoria"]),
    ("Stellenbosch University", &["stellenbosch university"]),
    ("UJ", &["university of johannesburg"]),
    ("UKZN", &["ukzn", "university of kwazulu-natal"]),
    ("Coursera", &["coursera"]),
    ("Udemy", &["udemy"]),
    // Retail and services
    ("Takealot", &["takealot"]),
    ("Checkers", &["checkers"]),
    ("Woolworths", &["woolworths"]),
    ("Pick n Pay", &["pick n pay", "picknpay"]),
    ("Shoprite", &["shoprite"]),
    ("Makro", &["makro"]),
    ("Game", &["game stores"]),
    ("Incredible Connection", &["incredible connection"]),
    ("Builders", &["builders warehouse"]),
    ("Mr Price", &["mr price"]),
    ("Uber", &["uber"]),
    ("Bolt", &["bolt.eu", "bolt ride"]),
    ("Netflix", &["netflix"]),
    ("Showmax", &["showmax"]),
    ("DStv", &["dstv", "multichoice"]),
    ("Spotify", &["spotify"]),
    ("Microsoft", &["microsoft"]),
    ("Google", &["google"]),
    ("Apple", &["apple inc", "apple distribution"]),
    ("Amazon", &["amazon"]),
    ("Adobe", &["adobe"]),
    ("Airbnb", &["airbnb"]),
    ("FlySafair", &["flysafair"]),
    ("Airlink", &["airlink"]),
    ("South African Airways", &["south african airways"]),
    ("Avis", &["avis"]),
    ("Toyota", &["toyota"]),
    ("Volkswagen", &["volkswagen"]),
];

/// True when `needle` occurs in `hay` (both lowercase) with no letter or digit either side.
pub fn has_word(hay: &str, needle: &str) -> bool {
    if needle.is_empty() {
        return false;
    }
    let mut start = 0;
    while let Some(pos) = hay[start..].find(needle) {
        let i = start + pos;
        let before = hay[..i].chars().next_back();
        let after = hay[i + needle.len()..].chars().next();
        let edge = |c: Option<char>| c.is_none_or(|c| !c.is_alphanumeric());
        if edge(before) && edge(after) {
            return true;
        }
        start = i + needle.len().max(1);
        while start < hay.len() && !hay.is_char_boundary(start) {
            start += 1;
        }
        if start >= hay.len() {
            break;
        }
    }
    false
}

/// The organisation a document is from, preferring names the owner approved before, then the
/// longest built-in match (so "Discovery Health" beats "Discovery").
pub fn find_org(text: &str, learned: &[String]) -> Option<String> {
    let hay = text.to_lowercase();
    if let Some(l) = learned.iter().find(|l| has_word(&hay, &l.to_lowercase())) {
        return Some(l.clone());
    }
    ORGS.iter()
        .flat_map(|(name, needles)| needles.iter().map(move |n| (*name, *n)))
        .filter(|(_, n)| has_word(&hay, n))
        .max_by_key(|(_, n)| n.len())
        .map(|(name, _)| name.to_string())
}
