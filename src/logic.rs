use log::info;
use serde_json::{json, Value};

pub(crate) fn info() -> Value {
    info!("INFO");

    json!({
        "apiversion": "1",
        "author": "luigicollesi",
        "color": "#C2410C",
        "head": "tiger-king",
        "tail": "mlh-gene",
    })
}

#[cfg(test)]
mod tests {
    use super::info;

    #[test]
    fn info_uses_battlesnake_api_v1() {
        let metadata = info();
        assert_eq!(metadata["apiversion"], "1");
        assert_eq!(metadata["author"], "luigicollesi");
        assert_eq!(metadata["color"], "#C2410C");
        assert_eq!(metadata["head"], "tiger-king");
        assert_eq!(metadata["tail"], "mlh-gene");
    }
}
