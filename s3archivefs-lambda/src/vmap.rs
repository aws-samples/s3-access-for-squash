use std::collections::HashMap;
use log::{debug, info};

const MAX_ENV_PREFIX_VMAP: usize = 20;

pub struct PrefixVMap {
    vmap: HashMap<String, String>,
    keys: Vec<String>,
}

impl PrefixVMap {

    pub fn new() -> Self {

        let mut vmap = HashMap::new();

        // load vmap from env
        for i in 1..=MAX_ENV_PREFIX_VMAP {
            let env_str = format!("S3ARCHIVEFS_PREFIX_VMAP{}", i);
            let env = std::env::var(env_str);
            if env.is_err() {
                break;
            }
            let prefix_vmap = env.unwrap();
            let pair: Vec<&str> = prefix_vmap.split('|').collect();
            if pair.len() != 2 {
                // if not a valid pair, skip it
                continue;
            }
            if let Some(val) = vmap.insert(pair[0].trim_start_matches('/').to_owned(), pair[1].to_owned()) {
                // key exist in vmap, skip it
                info!("ignore key: {} - val: {}, exist val: {} in vmap",
                    pair[0], pair[1], val);
                continue;
            }
        }

        // load vmap from file
        if let Ok(ext) = std::env::var("S3ARCHIVEFS_PREFIX_VMAP_EXT_FILE") {
            let ext_path = std::path::Path::new(&ext);
            if ext_path.exists() {
                if let Ok(text) = std::fs::read_to_string(ext_path) {
                    for line in text.lines() {
                        let pair: Vec<&str> = line.split('|').collect();
                        if pair.len() != 2 {
                            // if not a valid pair, skip it
                            continue;
                        }
                        if let Some(val) = vmap.insert(pair[0].trim_start_matches('/').to_owned(), pair[1].to_owned()) {
                            // key exist in vmap, skip it
                            info!("ignore key: {} - val: {}, exist val: {} in vmap",
                                pair[0], pair[1], val);
                            continue;
                        }
                    }
                }
            }
        }

        let mut keys = vmap.keys().clone().collect::<Vec<&String>>();
        keys.sort();
        debug!("keys: {:#?}", keys);
        Self {
            keys: keys.iter().map(|k| k.to_string()).collect(),
            vmap: vmap,
        }
    }

    // return: (matched_virtual_prefix, bucket, prefix, object)
    pub fn query(&self, prefix: &str) -> Option<(String, String, String, String)> {

        let null = "".to_string();
        let mut found = self.keys.iter().rfind(|&x| {
            if prefix.len() > x.len() {
                x.starts_with(prefix.split_at(x.len()).0)
            } else {
                x.starts_with(prefix)
            }
        });
        if found.is_none() {
            // check if has root mapping
            if self.vmap.contains_key("") {
                found = Some(&null);
            } else {
                return None;
            }
        }

        if let Some((key, val)) = self.vmap.get_key_value(found.unwrap()) {
            if let Ok(s3url) = url::Url::parse(val) {
                match s3url.scheme() {
                    "s3" | "S3" => {
                        let bucket = s3url.host_str();
                        if bucket.is_none() {
                            return None;
                        }
                        let object_key = s3url.path().trim_start_matches('/');
                        if let Some((object, prefix)) = object_key.split('/').collect::<Vec<&str>>().split_last() {
                            if object.is_empty() {
                                return None;
                            }
                            return Some((key.to_string(), bucket.unwrap().to_string(), prefix.join("/"), object.to_string()));
                        }

                    },
                    _ => {},
                }
            }
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_vmap() {
        std::env::set_var("S3ARCHIVEFS_PREFIX_VMAP_EXT_FILE", "/tmp/mapping.csv");
        std::env::set_var("S3ARCHIVEFS_PREFIX_VMAP3", "/virtual/prefix1|s3://ahabucket/prefix1/object.name");
        std::env::set_var("S3ARCHIVEFS_PREFIX_VMAP1", "virtual/prefix2|s3://ahabucket/prefix2/object.name");
        std::env::set_var("S3ARCHIVEFS_PREFIX_VMAP2", "/|s3://ahabucket/root/object.name");
        std::env::set_var("S3ARCHIVEFS_PREFIX_VMAP4", "/prefix3/subprefix3|s3://ahabucket/prefix3/object.name");
        std::env::set_var("S3ARCHIVEFS_PREFIX_VMAP5", "/prefix4/subprefix4/prefix|s3://ahabucket/prefix4/object.name");
        let vmap = PrefixVMap::new();

        println!("query virtual/prefix ->");
        println!("  {:?}", vmap.query("virtual/prefix"));

        println!("query virtual/prefix10 ->");
        println!("  {:?}", vmap.query("virtual/prefix10"));

        println!("query vir ->");
        println!("  {:?}", vmap.query("vir"));

        println!("query prefix6/ ->");
        println!("  {:?}", vmap.query("prefix6/"));

        println!("query prefix5 ->");
        println!("  {:?}", vmap.query("prefix5"));
    }
}
