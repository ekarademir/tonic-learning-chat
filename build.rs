fn main() -> Result<(), Box<dyn std::error::Error>> {

    println!("cargo:rustc-env={}={}", "RUST_LOG", "DEBUG");

    tonic_build::compile_protos("proto/chat.proto")?;
    Ok(())
}
