mod api;

#[cfg(not(feature = "std"))]
mod cabi_realloc;

#[cfg(not(feature = "std"))]
static mut ARENA: [u8; 10000] = [0; 10000];

#[cfg(not(feature = "std"))]
#[global_allocator]
static ALLOCATOR: talc::Talck<spin::Mutex<()>, talc::ClaimOnOom> = talc::Talc::new(unsafe {
    talc::ClaimOnOom::new(talc::Span::from_array(core::ptr::addr_of!(ARENA) as *mut [u8; 10000]))
})
.lock();

#[cfg(not(feature = "std"))]
#[panic_handler]
fn panic(_info: &core::panic::PanicInfo) -> ! { loop {} }
