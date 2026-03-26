fn main() {
    // Compile protobuf definitions
    let mut config = prost_build::Config::new();
    config.bytes(["."]);

    config
        .compile_protos(&["proto/event.proto"], &["proto/"])
        .expect("Failed to compile protobuf definitions");

    println!("cargo:rerun-if-changed=proto/event.proto");
    println!("cargo:rerun-if-changed=build.rs");
}
