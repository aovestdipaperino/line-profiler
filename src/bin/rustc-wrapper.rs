use anyhow::Result;
use line_profiler::rustc_wrapper::RustcWrapper;

fn main() -> Result<()> {
    let wrapper = RustcWrapper::from_env()?;
    let exit_code = wrapper.run()?;
    std::process::exit(exit_code);
}
