use std::io;

pub fn get_page_size() -> Result<usize, io::Error> {
    let page_size = unsafe { libc::sysconf(libc::_SC_PAGESIZE) };
    if page_size <= 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(page_size as usize)
}

pub fn round_up_to_page_size(n: usize) -> usize {
    let page_size = get_page_size().expect("could not get the system's page size");
    if n > 0 && n % page_size == 0 {
        return n;
    }
    (n / page_size + 1) * page_size
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_up_to_page_size() {
        let page_size = get_page_size().unwrap();
        println!("page size is {}", page_size);
        assert!(super::round_up_to_page_size(0) == page_size);
        assert!(super::round_up_to_page_size(1) == page_size);
        assert!(super::round_up_to_page_size(page_size - 1) == page_size);
        assert!(super::round_up_to_page_size(page_size) == page_size);
        assert!(super::round_up_to_page_size(page_size + 1) == page_size * 2);
        assert!(super::round_up_to_page_size(page_size * 2) == page_size * 2);
    }
}
