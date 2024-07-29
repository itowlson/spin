// // These are not in a lib so copy-pasting

// use std::path::PathBuf;

// use wasm_pkg_loader::{PackageRef, Version};

// pub enum WitSource {
//     Dir(PathBuf),
//     Registry(PackageRef, Version),
// }

// pub struct WitResolve {
//     pub wit: WitSource,
//     // Not sure if these are relevant to us - this is a wasm-tools CLI thing
//     pub features: Vec<String>,
//     pub all_features: bool,
// }

// impl WitResolve {
//     fn resolve_with_features(features: &[String], all_features: bool) -> wit_parser::Resolve {
//         let mut resolve = wit_parser::Resolve::default();
//         resolve.all_features = all_features;
//         for feature in features {
//             for f in feature.split_whitespace() {
//                 for f in f.split(',').filter(|s| !s.is_empty()) {
//                     resolve.features.insert(f.to_string());
//                 }
//             }
//         }
//         return resolve;
//     }

//     pub async fn load(&self) -> anyhow::Result<(wit_parser::Resolve, Vec<wit_parser::PackageId>)> {
//         let mut resolve = Self::resolve_with_features(&self.features, self.all_features);
//         let (pkg_ids, _) = match &self.wit {
//             WitSource::Dir(dir) => resolve.push_path(&dir)?,
//             WitSource::Registry(package_ref, version) => {
//                 let mut client = wasm_pkg_loader::Client::with_global_defaults()?;
//                 let release = client.get_release(package_ref, version).await?;
//                 client.stream_content(package_ref, &release).await?;  // okay but what do we do with the stream?  WAC WOULD HANDLE THIS NONSENSE FOR US
//                 todo!()
//             }
//         };
//         Ok((resolve, pkg_ids))
//     }
// }
