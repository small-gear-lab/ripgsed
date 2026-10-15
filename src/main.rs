#[global_allocator]
static ALLOCATOR: ripgsed::alloc::Guarded = ripgsed::alloc::Guarded;

fn main() {
    ripgsed::alloc::set_program_name(ripgsed::cli::program_name());
    std::process::exit(ripgsed::cli::main());
}
