use log::info;
use serde_json::{json, Value};

pub(crate) fn info() -> Value {
    info!("INFO");

    json!({
        "apiversion": "1",
        "author": "luigicollesi",
        "color": "#888888",
        "head": "default",
        "tail": "default",
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
    }
}
