from pathlib import Path
Path("crates/pigs/src/server.rs").write_text(
    Path(__file__).with_name("server_api_convert.rs.txt").read_text(encoding="utf-8")
    if Path(__file__).with_name("server_api_convert.rs.txt").exists()
    else open(0).read(),
    encoding="utf-8",
)
print("wrote server")
