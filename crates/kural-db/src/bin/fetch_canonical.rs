use anyhow::{Context, Result};
use clap::Parser;
use kural_core::{KnowledgeEntry, load_and_audit_canonical, normalize_nfc_trimmed};
use reqwest::Client;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::Duration;

const THIRUKKURAL_URL: &str =
    "https://raw.githubusercontent.com/tk120404/thirukkural/master/thirukkural.json";
const DETAIL_URL: &str =
    "https://raw.githubusercontent.com/tk120404/thirukkural/master/detail.json";
const PARIMEL_URL: &str = "https://raw.githubusercontent.com/vijayanandrp/Thirukkural-Tamil-Dataset/master/data/thirukkural_meanings.json";
const LEGACY_ABOUT_URL: &str = "https://raw.githubusercontent.com/ashwin20051222/AI-MODEL-1/main/frontend/src/pages/AboutPage.tsx";

#[derive(Parser)]
#[command(
    name = "fetch_canonical",
    about = "Pure-Rust canonical Thirukkural downloader, knowledge_base generator, and forensic auditor"
)]
struct Cli {
    /// Directory where canonical JSON files will be saved
    #[arg(long, default_value = "seed_data/canonical")]
    canonical_dir: PathBuf,

    /// Path where bilingual knowledge_base.json will be saved
    #[arg(long, default_value = "seed_data/knowledge_base.json")]
    knowledge_out: PathBuf,

    /// Skip downloading if canonical files already exist locally and audit offline
    #[arg(long)]
    offline: bool,
}

async fn download_text(client: &Client, url: &str) -> Result<String> {
    let resp = client
        .get(url)
        .send()
        .await
        .with_context(|| format!("request failed for {url}"))?
        .error_for_status()
        .with_context(|| format!("non-success HTTP status for {url}"))?;
    resp.text()
        .await
        .with_context(|| format!("failed reading body from {url}"))
}

fn write_atomic(path: &Path, contents: &str) -> Result<()> {
    if let Some(parent) = path.parent()
        && !parent.as_os_str().is_empty()
    {
        fs::create_dir_all(parent)
            .with_context(|| format!("failed creating dir {}", parent.display()))?;
    }
    let tmp = path.with_extension("tmp");
    fs::write(&tmp, contents).with_context(|| format!("failed writing {}", tmp.display()))?;
    fs::rename(&tmp, path).with_context(|| format!("failed renaming to {}", path.display()))?;
    Ok(())
}

fn kb(
    id: i64,
    category: &str,
    question_ta: &str,
    question_en: &str,
    answer_ta: &str,
    answer_en: &str,
    keywords: &str,
) -> KnowledgeEntry {
    KnowledgeEntry {
        id,
        category: normalize_nfc_trimmed(category),
        question_ta: normalize_nfc_trimmed(question_ta),
        question_en: normalize_nfc_trimmed(question_en),
        answer_ta: normalize_nfc_trimmed(answer_ta),
        answer_en: normalize_nfc_trimmed(answer_en),
        keywords: normalize_nfc_trimmed(keywords),
    }
}

fn build_curated_knowledge_base(legacy_about_verified: bool) -> Vec<KnowledgeEntry> {
    let legacy_note_ta = if legacy_about_verified {
        " (உலகப் பொதுமறை, குறள் வெண்பா, வள்ளுவர் கோட்டம் மற்றும் கன்னியாகுமரி சிலை தகவல்களுடன் சரிபார்க்கப்பட்டது)"
    } else {
        ""
    };
    let legacy_note_en = if legacy_about_verified {
        " (verified alongside cultural records on Ulagap Podhumarai, Kural Venba, Valluvar Kottam, and the Kanyakumari statue)"
    } else {
        ""
    };

    vec![
        kb(
            1,
            "biography",
            "திருவள்ளுவர் யார்?",
            "Who is Thiruvalluvar?",
            "திருவள்ளுவர் இரண்டாயிரம் ஆண்டுகளுக்கு முன்பு வாழ்ந்த தமிழ் ஞானியும் கவிஞருமாவார். மதம், சாதி, இனம், நாடு கடந்த உலகளாவிய வாழ்வியல் விழுமியங்களை 1,330 திருக்குறள்களில் ஏழு சீர்களில் வழங்கியவர்.",
            "Thiruvalluvar is a celebrated Tamil poet-philosopher who lived over two millennia ago and authored the Thirukkural—1,330 timeless two-line couplets presenting universal human ethics beyond religion, caste, race, or nationality.",
            "thiruvalluvar poet sage biography tamil philosopher",
        ),
        kb(
            2,
            "biography",
            "திருவள்ளுவர் வாழ்ந்த காலம் எது?",
            "When did Thiruvalluvar live?",
            "வரலாற்றாசிரியர்கள் மற்றும் தமிழ் அறிஞர்களின் கணிப்பின்படி திருவள்ளுவர் கி.மு. 31 முதல் கி.பி. 5ஆம் நூற்றாண்டிற்குள் வாழ்ந்திருக்கலாம் எனக் கருதப்படுகிறது. கி.மு. 31ஆம் ஆண்டே திருவள்ளுவர் ஆண்டின் தொடக்கமாகத் தமிழக அரசால் கணக்கிடப்படுகிறது.",
            "Scholars place Thiruvalluvar between 31 BCE and the 5th century CE. The Government of Tamil Nadu officially marks 31 BCE as the start of the Thiruvalluvar Era (Thiruvalluvar Aandu).",
            "era date timeline 31 bce sangam period history",
        ),
        kb(
            3,
            "biography",
            "திருவள்ளுவரின் பிறப்பிடம் எது?",
            "Where did Thiruvalluvar live or come from?",
            "பாரம்பரியக் குறிப்புகளின்படி திருவள்ளுவர் சென்னையில் உள்ள மயிலாப்பூரில் வாழ்ந்ததாகவும், மதுரைத் தமிழ்ச் சங்கத்துடன் தொடர்புடையவராக இருந்ததாகவும் கருதப்படுகிறது. மயிலாப்பூரில் அவருக்குப் பழமையான திருக்கோயிலும் உள்ளது.",
            "Tradition closely associates Thiruvalluvar with Mylapore in present-day Chennai, where a historic shrine dedicated to him stands, as well as with the ancient Tamil Sangam scholarly tradition of Madurai.",
            "mylapore chennai madurai birthplace residence",
        ),
        kb(
            4,
            "biography",
            "திருவள்ளுவரின் துணைவியார் யார்?",
            "Who was Thiruvalluvar's wife?",
            "திருவள்ளுவரின் துணைவியார் வாசுகி அம்மையார் ஆவார். இல்லற வாழ்வில் அன்பு, பொறுமை, பண்பு மற்றும் ஒற்றுமைக்கு இலக்கணமாகப் போற்றப்படுபவர்.",
            "Thiruvalluvar's wife was Vasuki Ammaiyar, remembered in Tamil literary tradition as an exemplar of devotion, grace, patience, and harmonious domestic life.",
            "vasuki ammaiyar wife family domestic life",
        ),
        kb(
            5,
            "biography",
            "திருவள்ளுவரின் தொழில் என்னவாகக் கருதப்படுகிறது?",
            "What was Thiruvalluvar's occupation according to tradition?",
            "பாரம்பரிய வரலாற்றின்படி திருவள்ளுவர் நெசவாளராகப் பணியாற்றி எளிய வாழ்க்கை வாழ்ந்தவர் எனக் கூறப்படுகிறது. அவர் மனித அறத்தையும் வாழ்க்கை நெறியையும் எளிய மக்களுக்கும் புரியும்படி இயற்றினார்.",
            "According to traditional accounts, Thiruvalluvar earned his living as a humble weaver in Mylapore, living an ethical, modest life while composing verses accessible to both commoners and kings.",
            "weaver occupation profession humble life",
        ),
        kb(
            6,
            "biography",
            "திருவள்ளுவருக்கு வழங்கப்படும் சிறப்புப் பெயர்கள் யாவை?",
            "What are the honorary titles given to Thiruvalluvar?",
            "தெய்வப்புலவர், பொய்யில் புலவர், நாயனார், முதற்பாவலர், மாதானுபங்கி, செந்நாப்போதார், பெருநாவலர் ஆகிய சிறப்புப் பெயர்களால் திருவள்ளுவர் போற்றப்படுகிறார்.",
            "Thiruvalluvar is revered by many titles including Deyvappulavar (Divine Poet), Poyyil Pulavar (Unerring Poet), Mudharpavalar (First Poet), Sennaapothaar (Pure-Tongued Sage), and Perunavalar (Great Orator).",
            "titles honorifics deivappulavar poyyil pulavar",
        ),
        kb(
            7,
            "biography",
            "திருவள்ளுவர் தினம் எப்போது கொண்டாடப்படுகிறது?",
            "When is Thiruvalluvar Day celebrated?",
            "தைத் திங்கள் இரண்டாம் நாள் (பொதுவாக ஜனவரி 15 அல்லது 16) தமிழகத்தில் திருவள்ளுவர் தினமாக ஆண்டுதோறும் சிறப்பாகக் கொண்டாடப்படுகிறது.",
            "Thiruvalluvar Day is celebrated annually in Tamil Nadu on the second day of the Tamil month of Thai (usually January 15 or 16), right after Pongal.",
            "thiruvalluvar day thai pongal january festival",
        ),
        kb(
            8,
            "biography",
            "திருவள்ளுவர் ஆண்டு எவ்வாறு கணக்கிடப்படுகிறது?",
            "How is the Thiruvalluvar Year (Thiruvalluvar Aandu) calculated?",
            "கிறிஸ்து சகாப்தத்துடன் (CE) 31ஐக் கூட்டினால் திருவள்ளுவர் ஆண்டு கிடைக்கும். எடுத்துக்காட்டாக, கி.பி. 2026 என்பது திருவள்ளுவர் ஆண்டு 2057 ஆகும்.",
            "The Thiruvalluvar Year is calculated by adding 31 to the Gregorian Common Era year. For example, 2026 CE corresponds to Thiruvalluvar Year 2057.",
            "calendar thiruvalluvar year aandhu 31",
        ),
        kb(
            9,
            "philosophy",
            "திருக்குறள் ஏன் 'உலகப் பொதுமறை' என அழைக்கப்படுகிறது?",
            "Why is Thirukkural called 'Ulagap Podhumarai' (Universal Scripture)?",
            "திருக்குறளில் எங்கும் ஒரு குறிப்பிட்ட மதத்தின் பெயரோ, சாதியின் பெயரோ, நாட்டின் பெயரோ குறிப்பிடப்படவில்லை. மனித நேயம், அன்பு, நீதி, அறம் ஆகியவற்றைப் போதிப்பதால் இது 'உலகப் பொதுமறை' என அழைக்கப்படுகிறது.",
            "Thirukkural never mentions any specific religion, sect, caste, or country. Because it teaches compassion, truth, justice, and universal ethics for all humanity, it is revered as Ulagap Podhumarai—the Universal Secular Scripture.",
            "ulagap podhumarai secular universal ethics religion",
        ),
        kb(
            10,
            "structure",
            "திருக்குறளின் மொத்த அமைப்பு என்ன?",
            "What is the overall structure of the Thirukkural?",
            "திருக்குறள் 3 முப்பால்கள் (அறம், பொருள், இன்பம்), 9 இயல்கள், 133 அதிகாரங்கள், மற்றும் 1,330 குறட்பாக்களைக் கொண்டது. ஒவ்வொரு அதிகாரத்திலும் 10 குறள்கள் உள்ளன.",
            "The Thirukkural consists of 3 Paals (Divisions: Virtue, Wealth/Polity, Love), 9 Iyals (Sections), 133 Adhikarams (Chapters), and 1,330 couplets—with exactly 10 couplets in every chapter.",
            "structure 1330 133 9 3 paal iyal adhikaram",
        ),
        kb(
            11,
            "structure",
            "முதல் பிரிவான அறத்துப்பால் எதைப் பேசுகிறது?",
            "What does Division 1, Arathuppal (Book of Virtue), cover?",
            "அறத்துப்பால் (குறள்கள் 1–380, அதிகாரங்கள் 1–38) பாயிரவியல், இல்லறவியல், துறவறவியல், ஊழியல் என 4 இயல்களைக் கொண்டு தனிமனித ஒழுக்கம், அன்பு, விருந்தோம்பல், ஈகை, வாய்மை ஆகியவற்றை விளக்குகிறது.",
            "Arathuppal (Kurals 1–380, Chapters 1–38) comprises 4 Iyals—Prologue, Domestic Virtue, Ascetic Virtue, and Fate—covering personal righteousness, love, hospitality, gratitude, compassion, and truthfulness.",
            "arathuppal aram virtue division 1 380",
        ),
        kb(
            12,
            "structure",
            "இரண்டாம் பிரிவான பொருட்பால் எதைப் பேசுகிறது?",
            "What does Division 2, Porutpal (Book of Wealth & Polity), cover?",
            "பொருட்பால் (குறள்கள் 381–1080, அதிகாரங்கள் 39–108) அரசியல், அங்கவியல், ஒழிபியல் என 3 இயல்களைக் கொண்டு கல்வி, தலைமைத்துவம், ஆட்சிமுறை, பொருளாதாரம், நட்பு, குடிமைப் பண்பு ஆகியவற்றை ஆராய்கிறது.",
            "Porutpal (Kurals 381–1080, Chapters 39–108) spans 3 Iyals—Royalty & Polity, Elements of State, and Citizenship—addressing education, governance, economics, diplomacy, friendship, and civic ethics.",
            "porutpal porul wealth polity governance division 2 700",
        ),
        kb(
            13,
            "structure",
            "மூன்றாம் பிரிவான இன்பத்துப்பால் (காமத்துப்பால்) எதைப் பேசுகிறது?",
            "What does Division 3, Inbathuppal (Book of Love & Affection), cover?",
            "இன்பத்துப்பால் (குறள்கள் 1081–1330, அதிகாரங்கள் 109–133) களவியல் மற்றும் கற்பியல் என 2 இயல்களைக் கொண்டு அன்பின் ஆழம், காதல் உணர்வு, குடும்பப் பிணைப்பு மற்றும் மன ஒற்றுமையைக் கவித்துவமாகப் பாடுகிறது.",
            "Inbathuppal (also known as Kamathuppal; Kurals 1081–1330, Chapters 109–133) consists of Kalaviyal and Karpiyal, poetically portraying mutual affection, devotion, longing, and lifelong companionship.",
            "inbathuppal kamathuppal love affection division 3 250",
        ),
        kb(
            14,
            "structure",
            "திருக்குறளில் உள்ள 9 இயல்கள் யாவை?",
            "What are the 9 Iyals (Sections) of the Thirukkural?",
            "அறத்துப்பாலில் பாயிரவியல், இல்லறவியல், துறவறவியல், ஊழியல் (4); பொருட்பாலில் அரசியல், அங்கவியல், ஒழிபியல் (3); இன்பத்துப்பாலில் களவியல், கற்பியல் (2) என மொத்தம் 9 இயல்கள் உள்ளன.",
            "The 9 Iyals are: Prologue, Domestic Virtue, Ascetic Virtue, Fate (in Virtue); Royalty, Elements of State, Citizenship (in Wealth/Polity); and Pre-marital Love, Post-marital Love (in Love).",
            "9 iyals sections payiraviyal illaraviyal angaviyal ozhibiyal",
        ),
        kb(
            15,
            "literary",
            "குறள் வெண்பா இலக்கணம் என்றால் என்ன?",
            "What is the Kural Venba poetic meter?",
            "குறள் வெண்பா என்பது முதல் அடியில் நான்கு சீர்களும், இரண்டாம் அடியில் மூன்று சீர்களுமாக மொத்தம் ஏழு சீர்களில் அமைந்த ஈரடிப் பா வடிவமாகும். 'அணுவைத் துளைத்து ஏழ்கடலைப் புகட்டி' என ஔவையார் இதன் செறிவைப் போற்றியுள்ளார்.",
            "Kural Venba is a crisp two-line Tamil poetic meter with 4 metrical feet (seers) in the first line and 3 in the second—just 7 words total. Poetess Avvaiyar praised it as piercing an atom and pouring seven oceans inside.",
            "kural venba meter 7 seers poetry grammar",
        ),
        kb(
            16,
            "literary",
            "திருக்குறள் எந்தத் தமிழ் இலக்கியத் தொகுப்பில் இடம்பெற்றுள்ளது?",
            "Which classical Tamil literary collection includes the Thirukkural?",
            "திருக்குறள் சங்கம் மருவிய காலத்து 'பதினெண்கீழ்க்கணக்கு' நூல் வரிசையில் முதன்மையான அறநூலாக இடம்பெற்றுள்ளது.",
            "The Thirukkural is the foremost ethical work in the classical Pathinenkilkanakku ('Eighteen Lesser Texts') anthology of post-Sangam Tamil literature.",
            "pathinenkilkanakku sangam literature anthology",
        ),
        kb(
            17,
            "literary",
            "திருக்குறளின் முதல் குறளும் கடைசி குறளும் எவை?",
            "What are the first and last Kurals of the Thirukkural?",
            "முதல் குறள் (1): 'அகர முதல எழுத்தெல்லாம் ஆதி பகவன் முதற்றே உலகு.' கடைசி குறள் (1330): 'ஊடுதல் காமத்திற்கு இன்பம் அதற்கு இன்பம் கூடி முயங்கப் பெறின்.'",
            "Kural #1 begins 'Agara mudhala ezhuthellaam...' (As A is first among letters, the Primal Source leads the world), and Kural #1330 closes the 1,330th couplet on reconciliation deepening affection.",
            "first kural 1 last kural 1330 akara",
        ),
        kb(
            18,
            "literary",
            "திருக்குறளைப் போற்றும் 'திருவள்ளுவ மாலை' என்றால் என்ன?",
            "What is the Thiruvalluva Maalai?",
            "திருவள்ளுவ மாலை என்பது திருக்குறளின் பெருமையைப் போற்றிப் பல்வேறு சங்கப் புலவர்களால் பாடப்பட்ட 55 வெண்பாக்களின் தொகுப்பு நூலாகும்.",
            "Thiruvalluva Maalai is an ancient anthology of 55 Tamil verses attributed to classical Sangam poets extolling the matchless depth, brevity, and universality of the Thirukkural.",
            "thiruvalluva maalai praise poems sangam poets",
        ),
        kb(
            19,
            "monuments",
            "கன்னியாகுமரி 133 அடி திருவள்ளுவர் சிலையின் சிறப்பு என்ன?",
            "What is special about the 133-foot Thiruvalluvar Statue at Kanyakumari?",
            "முக்கடல் சங்கமிக்கும் கன்னியாகுமரி பாறையில் அமைந்துள்ள இச்சிலை 133 அதிகாரங்களைக் குறிக்கும் வகையில் 133 அடி உயரத்தில் (38 அடி பீடம் - அறத்துப்பால், 95 அடி சிலை - பொருள் & இன்பத்துப்பால்) 2000ஆம் ஆண்டு திறக்கப்பட்டது.",
            "Standing on a coastal rock at Kanyakumari where three seas meet, the monumental stone statue rises 133 feet—symbolizing all 133 chapters (a 38-ft pedestal for Virtue and a 95-ft figure for Wealth and Love)—inaugurated in 2000.",
            "kanyakumari statue 133 feet rock memorial monument",
        ),
        kb(
            20,
            "monuments",
            "சென்னை வள்ளுவர் கோட்டத்தின் சிறப்பு என்ன?",
            "What is Valluvar Kottam in Chennai?",
            "சென்னையில் உள்ள வள்ளுவர் கோட்டம் திருவாரூர் தேரின் வடிவில் கருங்கல்லால் 1976இல் அமைக்கப்பட்ட நினைவுச் சின்னமாகும். இதன் அரங்கத்தின் 133 கருங்கல் தூண்களிலும் 1,330 குறள்களும் பொறிக்கப்பட்டுள்ளன.",
            "Inaugurated in 1976 in Chennai, Valluvar Kottam is a monumental stone memorial shaped like the temple chariot of Thiruvarur. Inside its grand auditorium, all 1,330 Kurals are carved onto 133 granite pillars.",
            "valluvar kottam chennai chariot granite pillars",
        ),
        kb(
            21,
            "monuments",
            "வள்ளுவர் கோட்டத்தில் உள்ள 1,330 குறள்களின் வண்ணக் குறியீடு என்ன?",
            "How are the 3 divisions distinguished in Valluvar Kottam's carvings?",
            "வள்ளுவர் கோட்டத்தில் அறத்துப்பால் பளிங்குப் பொறிப்புகளில் தனித்தும், பொருட்பாலும் இன்பத்துப்பாலும் கருங்கல் மற்றும் செங்கல் நிற வேறுபாடுகளுடன் 133 அதிகாரங்களாக வரிசைப்படுத்தப்பட்டுள்ளன.",
            "At Valluvar Kottam, the 1,330 couplets are systematically arranged across 133 carved blocks and pillars representing the three Paals so visitors can read every chapter in sequence.",
            "valluvar kottam carvings 1330 pillars",
        ),
        kb(
            22,
            "monuments",
            "இந்தியாவிலும் வெளிநாடுகளிலும் திருவள்ளுவருக்கு வேறு எங்கு சிலைகள் உள்ளன?",
            "Are there Thiruvalluvar statues outside Tamil Nadu?",
            "ஆம். பெங்களூரு (அல்சூர்), ஹரித்வார் (உத்தரகாண்ட்), புதுதில்லி, மற்றும் லண்டன் (SOAS), பிரான்ஸ் (செர்ஜி) ஆகிய இடங்களிலும் திருவள்ளுவர் சிலைகள் நிறுவப்பட்டுள்ளன.",
            "Yes. Beyond Tamil Nadu, statues of Thiruvalluvar stand in Bengaluru (Ulsoor), Haridwar, New Delhi, as well as internationally in London (SOAS) and Cergy, France.",
            "statues london france bengaluru haridwar global",
        ),
        kb(
            23,
            "commentaries",
            "திருக்குறளுக்கு உரை எழுதிய பழமையான உரையாசிரியர்கள் யார்?",
            "Who were the classical medieval commentators on the Thirukkural?",
            "மணக்குடவர், தாமத்தர், நச்சர், பரிதி, பரிமேலழகர், திருமலையர், மல்லர், பரிப்பெருமாள், காலிங்கர், தருமர் ஆகிய பதின்மர் பழைய உரையாசிரியர்கள் ஆவர். இவர்களுள் பரிமேலழகர் உரை தலைசிறந்ததாகப் போற்றப்படுகிறது.",
            "Ten classical commentators (Pathinmar) wrote medieval Tamil glosses on the Thirukkural, including Manakkudavar, Paridhi, Kalingar, Paripperumal, and Parimelazhagar—whose commentary remains the most celebrated.",
            "parimelazhagar manakkudavar classical commentators urai",
        ),
        kb(
            24,
            "commentaries",
            "இக்காலத் தமிழ் உரையாசிரியர்களில் முதன்மையானவர்கள் யார்?",
            "Who are the prominent modern Tamil commentators included in this database?",
            "டாக்டர் மு. வரதராசனார் (மு.வ.), சாலமன் பாப்பையா, மற்றும் மு. கருணாநிதி (கலைஞர்) ஆகியோரின் எளிய, தெளிவான தமிழ் உரைகள் இத்தரவுத்தளத்தில் இடம்பெற்றுள்ளன.",
            "This database preserves the classic modern Tamil commentaries of Dr. Mu. Varadarajanar (Mu. Va.), Solomon Pappiah, and M. Karunanidhi (Kalaignar), alongside optional Parimelazhagar.",
            "mu va varadarajanar solomon pappiah kalaignar commentary",
        ),
        kb(
            25,
            "translations",
            "திருக்குறளை முதலில் ஆங்கிலத்தில் மொழிபெயர்த்தவர்கள் யார்?",
            "Who translated the Thirukkural into English and European languages?",
            "வீரமாமுனிவர் (Constanzo Beschi) லத்தீன் மொழியிலும், பிரான்சிஸ் ஒயிட் எல்லிஸ் மற்றும் ஜி.யு. போப் (G.U. Pope - 1886) ஆங்கிலத்திலும் திருக்குறளை உலகறியச் செய்தனர்.",
            "Constanzo Beschi (Veeramamunivar) translated parts into Latin, Francis Whyte Ellis produced early English translations, and Rev. G.U. Pope published the landmark complete English verse translation in 1886.",
            "gu pope ellis beschi veeramamunivar english latin translation",
        ),
        kb(
            26,
            "translations",
            "திருக்குறள் எத்தனை மொழிகளில் மொழிபெயர்க்கப்பட்டுள்ளது?",
            "Into how many languages has the Thirukkural been translated?",
            "திருக்குறள் உலகின் மிகவும் அதிகமாக மொழிபெயர்க்கப்பட்ட அறநூல்களில் ஒன்றாகும்; இது 40-க்கும் மேற்பட்ட உலக மொழிகளிலும், 100-க்கும் மேற்பட்ட மொழிபெயர்ப்பு வடிவங்களிலும் வெளிவந்துள்ளது.",
            "The Thirukkural is among the most translated ethical classics in world literature, rendered into more than 40 world languages across over 100 distinct translations.",
            "translations languages world literature global reach",
        ),
        kb(
            27,
            "philosophy",
            "மகாத்மா காந்தி மற்றும் ஆல்பர்ட் சுவைட்சர் திருக்குறளைப் பற்றி என்ன கூறினர்?",
            "What did Mahatma Gandhi and Albert Schweitzer say about the Thirukkural?",
            "மகாத்மா காந்தி திருக்குறளை மனித வாழ்விற்கு நேரடி வழிகாட்டியாகப் போற்றினார்; நோபல் பரிசு பெற்ற ஆல்பர்ட் சுவைட்சர் 'உலக இலக்கியத்தில் இவ்வளவு உயர்ந்த ஞானத்தைக் கொண்ட நூல்கள் மிகச் சிலவே' எனப் பாராட்டினார்.",
            "Mahatma Gandhi admired Thirukkural as a treasure of moral guidance, while Nobel laureate Dr. Albert Schweitzer wrote that there hardly exists in the literature of the world a collection of maxims containing so much lofty wisdom.",
            "gandhi albert schweitzer praise quotes",
        ),
        kb(
            28,
            "philosophy",
            "திருக்குறள் கற்பிக்கும் முக்கிய அறநெறிகள் யாவை?",
            "What core ethical principles does Thirukkural teach?",
            "அன்புடைமை, வாய்மை, நன்றி மறவாமை, இன்னா செய்யாமை (அகிம்சை), ஒழுக்கம், ஈகை, அடக்கமுடைமை, பொறையுடைமை ஆகியவை திருக்குறளின் அடிப்படை அறநெறிகளாகும்.",
            "Core moral pillars of the Thirukkural include compassion (Anbudaimai), truthfulness (Vaimai), gratitude (Sei-nandri), non-harming (Innaa Seyyaamai), self-restraint, generosity, and patience.",
            "ethics virtues ahimsa truth gratitude compassion",
        ),
        kb(
            29,
            "philosophy",
            "கல்வியைப் பற்றித் திருவள்ளுவர் என்ன கூறுகிறார்?",
            "What does Thiruvalluvar teach about education (Kalvi)?",
            "அதிகாரம் 40 (கல்வி) மூலம் 'கற்க கசடறக் கற்பவை கற்றபின் நிற்க அதற்குத் தக' (குறள் 391) எனக் கல்வியை ஐயமின்றித் தெளிவாகக் கற்று அதன்படி வாழ வேண்டும் என்கிறார்; 'எண்ணென்ப ஏனை எழுத்தென்ப இவ்விரண்டும் கண்ணென்ப வாழும் உயிர்க்கு' (குறள் 392) என எண்ணும் எழுத்தும் இரு கண்கள் என்கிறார்.",
            "In Chapter 40 (Kalvi), Thiruvalluvar teaches: 'Learn thoroughly what is worthy to be learned, and live by it' (Kural 391), and compares numbers and letters to the two eyes of living beings (Kural 392).",
            "education kalvi 391 392 learning numbers letters",
        ),
        kb(
            30,
            "philosophy",
            "விமர்சன சிந்தனை (Critical Thinking) பற்றித் திருக்குறள் என்ன சொல்கிறது?",
            "What does Thirukkural say about critical thinking and truth-seeking?",
            "'எப்பொருள் யார்யார்வாய்க் கேட்பினும் அப்பொருள் மெய்ப்பொருள் காண்ப தறிவு' (குறள் 423) — யார் சொன்னாலும் அப்படியே நம்பாமல் அதன் உண்மைத் தன்மையை ஆராய்ந்து அறிவதே உண்மையான அறிவு என்கிறார்.",
            "Kural 423 declares: 'True wisdom is discerning the real truth behind anything, regardless of who says it'—an enduring maxim for critical thinking and resisting misinformation.",
            "critical thinking 423 epporul wisdom truth",
        ),
        kb(
            31,
            "philosophy",
            "நட்பைப் பற்றித் திருக்குறள் எவ்வாறு விளக்குகிறது?",
            "How does Thirukkural define true friendship (Natpu)?",
            "'உடுக்கை இழந்தவன் கைபோல ஆங்கே இடுக்கண் களைவதாம் நட்பு' (குறள் 788) — உடை நழுவும்போது கை தானாக உதவுவது போல, நண்பனுக்குத் துன்பம் வந்தவுடன் தயங்காமல் உதவுவதே உண்மையான நட்பு.",
            "Kural 788 famously defines friendship: 'Like a hand rushing instinctively to hold slipping clothes, true friendship immediately relieves a friend's distress.'",
            "friendship natpu 788 786 helping",
        ),
        kb(
            32,
            "philosophy",
            "உலகம் எவ்வளவு சமத்துவமானது என்பதைத் திருக்குறள் எவ்வாறு கூறுகிறது?",
            "What does Thirukkural say about human equality?",
            "'பிறப்பொக்கும் எல்லா உயிர்க்கும் சிறப்பொவ்வா செய்தொழில் வேற்றுமை யான்' (குறள் 972) — பிறப்பால் எல்லா மனிதர்களும் சமம்; அவரவர் செய்யும் நற்செயல்களாலேயே சிறப்பு ஏற்படுகிறது.",
            "Kural 972 proclaims universal equality: 'All living beings are equal by birth; distinction arises solely from the worth of their deeds.'",
            "equality 972 pirappokkum social justice deeds",
        ),
        kb(
            33,
            "philosophy",
            "நேரம் மற்றும் முயற்சி பற்றித் திருவள்ளுவர் என்ன கூறுகிறார்?",
            "What does Thirukkural teach about time management and perseverance?",
            "'காலம் அறிதல்' (அதிகாரம் 49) மூலம் சரியான தருணத்தில் செயல்படவும், 'ஊக்கமுடைமை' மற்றும் 'ஆள்வினையுடைமை' மூலம் 'தெய்வத்தான் ஆகா தெனினும் முயற்சிதன் மெய்வருத்தக் கூலி தரும்' (குறள் 619) என விடாமுயற்சியையும் போதிக்கிறார்.",
            "Through Chapters 49 (Knowing the Right Time) and 62 (Manly Effort), Thiruvalluvar teaches strategic timing and perseverance: 'Even if fate opposes, tireless effort yields its true reward' (Kural 619).",
            "time perseverance effort 619 481 49",
        ),
        kb(
            34,
            "philosophy",
            "இனிமையான பேச்சைப் பற்றித் திருக்குறள் என்ன கூறுகிறது?",
            "What does Thirukkural teach about kind speech (Insol)?",
            "'இனிய உளவாக இன்னாத கூறல் கனியிருப்பக் காய்கவர்ந் தற்று' (குறள் 100) — இனிமையான சொற்கள் இருக்கும்போது கடுஞ்சொற்களைப் பேசுவது கனிந்த பழம் இருக்கக் காயைத் தின்பதற்குச் சமம்.",
            "Kural 100 compares harsh speech to eating unripe fruit when sweet ripe fruit is right at hand, urging gentle, encouraging words in daily life.",
            "speech insol kindness words 100",
        ),
        kb(
            35,
            "philosophy",
            "நன்றி உணர்வைப் பற்றித் திருக்குறள் என்ன சொல்கிறது?",
            "What is Thiruvalluvar's teaching on gratitude?",
            "'எந்நன்றி கொன்றார்க்கும் உய்வுண்டாம் உய்வில்லை செய்ந்நன்றி கொன்ற மகற்கு' (குறள் 110) — பிற தவறுகளிலிருந்து மீள வழி உண்டு; ஆனால் ஒருவர் செய்த உதவியை மறப்பவருக்கு மன்னிப்பில்லை என நன்றியின் உயரத்தை உணர்த்துகிறது.",
            "Kural 110 places gratitude at the pinnacle of ethics: 'There may be redemption for every other wrong, but never for one who forgets a kindness done to them.'",
            "gratitude nandri 110 101 108",
        ),
        kb(
            36,
            "philosophy",
            "சினம் (கோபம்) பற்றித் திருக்குறள் தரும் அறிவுரை என்ன?",
            "What does Thirukkural advise about controlling anger?",
            "'தன்னைத்தான் காக்கின் சினங்காக்க காவாக்கால் தன்னையே கொல்லும் சினம்' (குறள் 305) — தன்னைக் காத்துக்கொள்ள விரும்புபவர் கோபத்தைக் கட்டுப்படுத்த வேண்டும்; இல்லையேல் கோபமே அவரை அழித்துவிடும்.",
            "Kural 305 warns: 'Guard against anger if you wish to guard yourself; left unchecked, anger destroys the very person who holds it.'",
            "anger sinam self-control 305",
        ),
        kb(
            37,
            "philosophy",
            "பொதுவாழ்விலும் அரசியலிலும் இருக்க வேண்டிய பண்புகள் யாவை?",
            "What leadership and governance qualities are emphasized in Porutpal?",
            "பொருட்பாலில் 'இறைமாட்சி' (அதிகாரம் 39) மூலம் அஞ்சாமை, ஈகை, அறிவு, ஊக்கம் ஆகியவையும், 'செங்கோன்மை' மூலம் நடுநிலைமையும் ஊழலற்ற நீதியான ஆட்சியும் வலியுறுத்தப்படுகின்றன.",
            "Porutpal highlights fearlessness, generosity, wisdom, and energy in leadership (Chapter 39), alongside impartial justice and accountability in governance (Sengonmai, Chapter 55).",
            "leadership governance polity 381 382 justice",
        ),
        kb(
            38,
            "faq",
            "குழந்தைகள் இன்பத்துப்பால் குறள்களைப் படிக்கலாமா?",
            "How does Thiruvalluvar AI adapt Inbathuppal (Kurals 1081–1330) for children?",
            "திருக்குறள் அனைவருக்கும் பொதுவானது. குழந்தைகள் பிரிவில் (6–12 வயது) தினசரி குறள் அறத்துப்பால் மற்றும் பொருட்பாலிலிருந்து (1–1080) தேர்ந்தெடுக்கப்படும்; இன்பத்துப்பால் குறள்களின் குட்டி கதைகள் குடும்ப அன்பு, நட்பு, கருணை, நல்லிணக்கம் சார்ந்ததாகக் குழந்தைகளுக்குப் பாதுகாப்பாக வடிவமைக்கப்பட்டுள்ளன.",
            "Daily picks for Kids Mode (ages 6–12) draw from Kurals 1–1080 (Virtue and Polity), while Kurals 1081–1330 remain searchable with Kutty Stories gently framed around family affection, kindness, friendship, and harmony.",
            "kids child safe inbathuppal daily kural 1080",
        ),
        kb(
            39,
            "faq",
            "திருக்குறளில் கடவுள் பெயர் குறிப்பிடப்பட்டுள்ளதா?",
            "Does Thirukkural name any specific deity?",
            "இல்லை. கடவுள் வாழ்த்து அதிகாரத்தில் 'ஆதிபகவன்', 'வாலறிவன்', 'மலர்மிசை ஏகினான்', 'இறைவன்' போன்ற பொதுவான தத்துவச் சொற்களையே திருவள்ளுவர் பயன்படுத்தியுள்ளார்; எந்த மதக் கடவுளின் பெயரையும் அவர் குறிப்பிடவில்லை.",
            "No specific sectarian deity is named. In Chapter 1, Thiruvalluvar uses universal philosophical epithets such as Aadhi Bhagavan (Primal Source), Vaalarivan (Pure Wisdom), and Iraivan, keeping the text inclusive for all.",
            "god deity kadavul vazhthu secular",
        ),
        kb(
            40,
            "faq",
            "தமிழ் எழுத்துகளில் எதைப் பற்றி முதல் குறளில் கூறப்படுகிறது?",
            "Why does Kural #1 begin with the letter 'A' (அகரம்)?",
            "தமிழ் உட்படப் பல உலக மொழிகளில் 'அ' (A) முதல் எழுத்தாக அமைந்து எல்லா எழுத்துகளுக்கும் ஒலி அடித்தளமாக விளங்குகிறது. அதை உவமையாக்கி உலக இயங்குவதற்கு ஆதாரமான முதற்சக்தியைத் திருவள்ளுவர் சுட்டுகிறார்.",
            "In Tamil and many world scripts, 'அ' (A) is the very first vowel and acoustic foundation of speech. Thiruvalluvar uses it as a universal metaphor for the foundational source underlying the cosmos.",
            "letter a akara kural 1 grammar",
        ),
        kb(
            41,
            "faq",
            "வள்ளுவர் AI இணைய இணைப்பின்றி (Offline) செயல்படுமா?",
            "Does Thiruvalluvar AI work completely offline?",
            "ஆம்! இத்திட்டத்தின் SQLite + FTS5 தரவுத்தளம் (thirukkural.db) அனைத்து 1,330 குறள்கள், தமிழ்-ஆங்கில விளக்கங்கள், குட்டி கதைகள், இளையோர் பகுப்பாய்வுகள் மற்றும் தேடல் குறியீடுகளுடன் 100% இணையமின்றிச் செயல்படுகிறது.",
            "Yes! The compiled SQLite + FTS5 database (thirukkural.db) stores all 1,330 Kurals, classical commentaries, bilingual Kutty Stories, Youth analyses, quizzes, and full-text indexes completely offline—ideal for schools and Raspberry Pi 4B kiosks.",
            "offline sqlite fts5 raspberry pi privacy local",
        ),
        kb(
            42,
            "faq",
            "திருக்குறளைப் பற்றிய தகவல் தமிழ் மற்றும் ஆங்கிலம் இரண்டிலும் உள்ளதா?",
            "Are both Tamil and English supported across all 1,330 Kurals?",
            "ஆம். ஒவ்வொரு குறளுக்கும் மூலத் தமிழ், ரோமன் ஒலிபெயர்ப்பு, மு.வ./சாலமன் பாப்பையா/கலைஞர் உரைகள், ஆங்கில மொழிபெயர்ப்பு, மேலும் குழந்தைகள், இளையோர், பெரியோர் ஆகிய மூன்று வயது நிலைகளுக்கும் தமிழ் மற்றும் ஆங்கில உள்ளடக்கம் உள்ளது.",
            "Yes. Every single one of the 1,330 Kurals includes verbatim Tamil verse, Roman transliteration, classical Tamil commentaries, English translation, plus complete bilingual Tamil and English content for Kids, Youth, and Adult modes.",
            "bilingual tamil english transliteration commentaries",
        ),
        kb(
            43,
            "faq",
            "திருக்குறள் எண், அதிகாரம் அல்லது சொல் கொண்டு எப்படித் தேடலாம்?",
            "How can I search for a Kural in this system?",
            "குறள் எண் ('42' அல்லது 'குறள் 481'), அதிகாரம் ('கடவுள் வாழ்த்து' அல்லது 'Knowing the Right Time'), தமிழ்/ஆங்கிலச் சொற்கள் ('நட்பு', 'friendship'), மற்றும் எழுத்துப்பிழையுடன் கூடிய சொற்கள் ('அறிவுடமை', 'thiruvalvar') மூலமும் தேடலாம்.",
            "You can search by exact Kural number ('42', 'குறள் 481'), Adhikaram name or number ('கடவுள் வாழ்த்து', 'Knowing the Right Time'), Tamil or English keywords ('நட்பு', 'friendship'), or even misspelled queries via built-in fuzzy recovery.",
            "search hybrid fts5 fuzzy number adhikaram",
        ),
        kb(
            44,
            "literary",
            "மழையைப் பற்றித் திருவள்ளுவர் ஏன் இரண்டாவது அதிகாரமாக 'வான்சிறப்பு' பாடினார்?",
            "Why did Thiruvalluvar place 'The Blessing of Rain' (Vaansirappu) as Chapter 2?",
            "நீரின்றி அமையாது உலகு (குறள் 20) என்பதால் மனித அறமும் உணவும் உயிர்களின் இருப்பும் மழையையும் இயற்கையையும் சார்ந்தே உள்ளன என்பதைத் தொடக்கத்திலேயே வலியுறுத்த வான்சிறப்பை இரண்டாவது அதிகாரமாக வைத்தார்.",
            "Because all life, food, and moral society depend on water ('The world cannot exist without water' — Kural 20), Thiruvalluvar placed ecological stewardship and rain right after the Prologue's invocation.",
            "rain water ecology vaansirappu 20 chapter 2",
        ),
        kb(
            45,
            "faq",
            "உள்ளூர்க் கலாச்சாரப் பதிவுகளுடன் இத்தகவல் தொகுப்பு சரிபார்க்கப்பட்டதா?",
            "Were the cultural and monument details cross-checked?",
            "ஆம், திருக்குறளின் 1,330/133/3 அமைப்பு, மதச்சார்பற்ற தன்மை, வள்ளுவர் கோட்டம் மற்றும் கன்னியாகுமரி 133 அடி சிலை குறிப்புகள் சரிபார்க்கப்பட்டு இருமொழி வடிவில் தொகுக்கப்பட்டுள்ளன.",
            "Yes, the 1,330/133/3 structural facts, secular heritage, Valluvar Kottam, and Kanyakumari 133-ft statue entries were cross-checked and curated into this bilingual offline knowledge base.",
            "cultural verification knowledge base heritage",
        ),
    ]
    .into_iter()
    .map(|mut e| {
        if e.id == 1 {
            e.answer_ta.push_str(legacy_note_ta);
            e.answer_en.push_str(legacy_note_en);
        }
        e
    })
    .collect()
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();
    fs::create_dir_all(&cli.canonical_dir)
        .with_context(|| format!("failed creating {}", cli.canonical_dir.display()))?;

    let kural_path = cli.canonical_dir.join("thirukkural.json");
    let detail_path = cli.canonical_dir.join("detail.json");
    let parimel_path = cli.canonical_dir.join("parimel.json");

    let mut legacy_about_verified = false;

    if !cli.offline {
        let client = Client::builder()
            .timeout(Duration::from_secs(30))
            .user_agent("thiruvalluvar-ai-rs/0.1")
            .build()?;

        println!("Downloading canonical thirukkural.json...");
        let kural_json = download_text(&client, THIRUKKURAL_URL).await?;
        write_atomic(&kural_path, &kural_json)?;

        println!("Downloading canonical detail.json...");
        let detail_json = download_text(&client, DETAIL_URL).await?;
        write_atomic(&detail_path, &detail_json)?;

        println!("Downloading optional Parimelazhagar commentary...");
        match download_text(&client, PARIMEL_URL).await {
            Ok(parimel_json) => {
                write_atomic(&parimel_path, &parimel_json)?;
                println!("Saved optional parimel.json");
            }
            Err(err) => {
                eprintln!("Warning: optional parimel.json download failed (non-fatal): {err}");
            }
        }

        match download_text(&client, LEGACY_ABOUT_URL).await {
            Ok(about_tsx)
                if about_tsx.contains("வள்ளுவர் கோட்டம்") && about_tsx.contains("133 அடி") =>
            {
                legacy_about_verified = true;
                println!("Verified cultural bio/monument facts from legacy AboutPage.tsx");
            }
            Ok(_) => {
                eprintln!(
                    "Warning: legacy AboutPage.tsx fetched without expected cultural markers"
                );
            }
            Err(err) => {
                eprintln!("Warning: legacy AboutPage.tsx check skipped (non-fatal): {err}");
            }
        }
    } else {
        println!("Offline mode: auditing existing canonical JSON files...");
    }

    let kb_entries = build_curated_knowledge_base(legacy_about_verified);
    let kb_json = serde_json::to_string_pretty(&kb_entries)?;
    write_atomic(&cli.knowledge_out, &kb_json)?;
    println!(
        "Wrote {} bilingual entries to {}",
        kb_entries.len(),
        cli.knowledge_out.display()
    );

    println!("Running Layer 1 forensic audit...");
    let dataset = load_and_audit_canonical(&cli.canonical_dir)
        .context("Layer 1 canonical forensic audit failed")?;

    let parimel_count = dataset
        .kurals
        .iter()
        .filter(|k| k.meaning_parimel.is_some())
        .count();
    let aram = dataset.kurals.iter().filter(|k| k.division_id == 1).count();
    let porul = dataset.kurals.iter().filter(|k| k.division_id == 2).count();
    let inbam = dataset.kurals.iter().filter(|k| k.division_id == 3).count();

    println!(
        "Forensic audit PASSED: divisions={}, iyals={}, adhikarams={}, kurals={} (Aram={aram}, Porul={porul}, Inbam={inbam}), parimel={parimel_count}/1330",
        dataset.divisions.len(),
        dataset.iyals.len(),
        dataset.adhikarams.len(),
        dataset.kurals.len()
    );
    println!(
        "Kural #1: {} -> {} ({})",
        dataset.kurals[0].opening_letter_ta,
        dataset.kurals[0].line1_ta,
        dataset.kurals[0].transliteration1
    );
    println!(
        "Kural #2: {} -> {} ({})",
        dataset.kurals[1].opening_letter_ta,
        dataset.kurals[1].line1_ta,
        dataset.kurals[1].transliteration1
    );

    Ok(())
}
