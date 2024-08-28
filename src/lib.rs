use std::cmp;
use std::ffi::CString;
use std::io::{Error, ErrorKind};
use std::process;

// TODO custom errors
// TODO example usage with UDS + a frame and a streaming codec
// TODO test mirroring

fn get_page_size() -> Result<usize, Error> {
    let page_size = unsafe { libc::sysconf(libc::_SC_PAGESIZE) };
    if page_size <= 0 {
        return Err(Error::last_os_error());
    }
    Ok(page_size as usize)
}

fn round_up_to_page_size(n: usize) -> usize {
    let page_size = get_page_size().expect("could not get the system's page size");
    if n > 0 && n % page_size == 0 {
        return n;
    }
    (n / page_size + 1) * page_size
}

pub struct MirroredBuffer<'a> {
    name: CString,

    head: usize,
    tail: usize,

    size_total: usize,
    size_mask: usize,
    size_used: usize,

    slice: &'a mut [u8],
}

impl<'a> MirroredBuffer<'a> {
    fn new(
        size: usize,
        name_suffix: Option<&str>,
        initial_value: Option<u8>,
    ) -> Result<MirroredBuffer<'a>, Error> {
        if size == 0 {
            // XXX: should have a custom error type
            return Err(Error::new(ErrorKind::Other, "invalid size"));
        }

        let name;
        if let Some(suffix) = name_suffix {
            name = format!("/mirrored-buffer-{}-{}", process::id(), suffix);
        } else {
            name = format!("/mirrored-buffer-{}", process::id());
        }

        let name = CString::new(name.as_str()).expect(
            format!(
                "invalid name: {} - contains a 0-byte when it should not",
                name
            )
            .as_str(),
        );

        let fd = unsafe {
            libc::shm_open(
                name.as_ptr(),
                libc::O_CREAT | libc::O_EXCL | libc::O_RDWR | libc::O_TRUNC,
                libc::S_IRUSR | libc::S_IWUSR,
            )
        };
        if fd == -1 {
            return Err(Error::last_os_error());
        }

        let size_total = round_up_to_page_size(size);
        let size_mask = size_total - 1;

        if size_total & size_mask != 0 {
            return Err(Error::new(ErrorKind::Other, "invalid page size"));
        }

        if unsafe { libc::ftruncate(fd, size_total as libc::off_t) } == -1 {
            return Err(Error::last_os_error());
        }

        let addr = unsafe {
            libc::mmap(
                std::ptr::null_mut(),
                size_total * 2,
                libc::PROT_NONE,
                libc::MAP_PRIVATE | libc::MAP_ANONYMOUS | libc::MAP_POPULATE,
                -1,
                0,
            )
        };
        if addr == libc::MAP_FAILED {
            return Err(Error::last_os_error());
        }

        let remap = |addr: *mut libc::c_void| -> Result<(), Error> {
            let ret = unsafe {
                libc::mmap(
                    addr,
                    size_total,
                    libc::PROT_READ | libc::PROT_WRITE,
                    libc::MAP_SHARED | libc::MAP_FIXED,
                    fd,
                    0,
                )
            };

            if ret == libc::MAP_FAILED {
                return Err(Error::last_os_error());
            }
            Ok(())
        };

        remap(addr)?;
        remap(unsafe { addr.byte_add(size_total) })?;

        let slice = unsafe { std::slice::from_raw_parts_mut(addr as *mut u8, size_total * 2) };

        if let Some(v) = initial_value {
            slice.fill(v);
        }

        Ok(MirroredBuffer {
            name,

            head: 0,
            tail: 0,

            size_total,
            size_mask,
            size_used: 0,

            slice,
        })
    }

    pub fn name(&self) -> &str {
        self.name.to_str().unwrap()
    }

    pub fn free(&self) -> usize {
        self.size_total - self.size_used
    }

    pub fn used(&self) -> usize {
        self.size_used
    }

    pub fn size(&self) -> usize {
        self.size_total
    }

    pub fn claim(&mut self, mut size: usize) -> Option<&mut [u8]> {
        size = cmp::min(size, self.free());
        if size == 0 {
            return None;
        }
        return Some(&mut self.slice[self.tail..(self.tail + size)]);
    }

    pub fn commit(&mut self, mut size: usize) -> usize {
        size = cmp::min(size, self.free());
        self.size_used += size;
        self.tail = (self.tail + size) & self.size_mask;
        return size;
    }

    pub fn consume(&mut self, mut size: usize) -> usize {
        size = cmp::min(size, self.used());
        self.size_used -= size;
        self.head = (self.head + size) & self.size_mask;
        return size;
    }

    pub fn committed(&self) -> Option<&[u8]> {
        if self.used() == 0 {
            return None;
        }

        if self.head < self.tail {
            return Some(&self.slice[self.head..self.tail]);
        }
        return Some(&self.slice[self.head..(self.tail + self.size())]);
    }
}

impl<'a> Drop for MirroredBuffer<'a> {
    fn drop(&mut self) {
        println!("dropped");
        if unsafe { libc::shm_unlink(self.name.as_ptr()) } != 0 {
            panic!("{}", Error::last_os_error());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::MirroredBuffer;

    // Used to prevent opening a MirroredBuffer on an already existing one,
    // which results in an error as the underlying tmpfs file is opened in
    // O_EXCL mode. O_EXCL ensures shm_open fails if the underlying file
    // already exists.
    //
    // This can happen if we destroy a MirroredBuffer and then quickly create
    // a new one with the same exact name. It is a result of calling shm_unlink
    // when Dropping the MirroredBuffer - the syscall might take a some time to
    // complete, notably more than it takes the binary to go to the next test
    // and create a new MirroredBuffer with the same name.
    //
    // As a result, each test creates a unique MirroredBuffer by providing the
    // return value of `next_buffer_index()` as a suffix.
    static mut BUFFER_INDEX: i32 = 0;

    fn next_buffer_index() -> String {
        let index = unsafe { BUFFER_INDEX };
        unsafe { BUFFER_INDEX += 1 };
        return index.to_string();
    }

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

    #[test]
    fn mirrored_buffer_new() {
        let buf = MirroredBuffer::new(0, Some(&next_buffer_index()), None);
        assert!(buf.is_err());

        let page_size = get_page_size().unwrap();
        let buf = MirroredBuffer::new(page_size, Some(&next_buffer_index()), None).unwrap();

        assert!(buf.name().contains("mirrored-buffer"));
        assert!(buf.head == 0);
        assert!(buf.tail == 0);
        assert!(buf.size_total == page_size);
        assert!(buf.size_mask > 0);
        assert!(buf.size_mask == page_size - 1);
        assert!(buf.size_total & buf.size_mask == 0);
        assert!(buf.size() == page_size);
        assert!(buf.used() == 0);
        assert!(buf.free() == page_size);
    }

    #[test]
    fn mirrored_buffer_claim_commit_consume() {
        let page_size = get_page_size().unwrap();
        let mut buf = MirroredBuffer::new(page_size, Some(&next_buffer_index()), Some(0)).unwrap();

        assert!(buf.claim(0) == None);

        // claim, head and tail do not change
        let claim_size = page_size / 2;
        {
            assert!(claim_size > 0);
            let claimed = buf.claim(claim_size);
            assert!(claimed.is_some());
            let claimed = claimed.unwrap();
            claimed.iter().all(|&x| x == 0);
            claimed.fill(1);
        }
        assert!(buf.head == 0);
        assert!(buf.tail == 0);
        assert!(buf.used() == 0);
        assert!(buf.free() == page_size);

        // commit, tail advances
        assert!(buf.commit(claim_size) == claim_size);
        assert!(buf.tail == claim_size);
        assert!(buf.head < buf.tail);
        assert!(buf.used() == claim_size);
        assert!(buf.free() == buf.size() - claim_size);

        // consume, head advances
        assert!(buf.consume(claim_size) == claim_size);
        assert!(buf.head == buf.tail);
        assert!(buf.head == claim_size);
        assert!(buf.used() == 0);
        assert!(buf.free() == buf.size());

        // now we force the ring buffer to wrap by claiming bast the end
        assert!(buf.head == buf.tail && buf.head > 0); // ensure we wrap
        let head_before = buf.head;
        {
            let claimed = buf.claim(page_size);
            assert!(claimed.is_some());
            let claimed = claimed.unwrap();
            claimed.fill(2);
        }
        assert!(buf.head == head_before);
        assert!(buf.tail == head_before);
        assert!(buf.used() == 0);
        assert!(buf.free() == buf.size());

        assert!(buf.commit(page_size) == page_size);
        assert!(buf.tail == page_size / 2);
        assert!(buf.head == buf.tail);
        assert!(buf.used() == page_size);
        assert!(buf.free() == 0);
        assert!(buf.claim(1).is_none());

        assert!(buf.slice.iter().all(|&x| x == 2));
        assert!(buf
            .committed()
            .is_some_and(|slice| slice.iter().all(|&x| x == 2)));
    }

    #[test]
    fn mirrored_buffer_claim_commit_consume_random() {
        use rand::Rng;
        let mut rng = rand::thread_rng();

        let page_size = get_page_size().unwrap();
        let mut buf = MirroredBuffer::new(page_size, Some(&next_buffer_index()), Some(0)).unwrap();

        let mut wrapped = 0;
        let mut it = 0;

        while wrapped < 12 {
            let size = rng.gen_range(0..page_size * 2);
            if let Some(slice) = buf.claim(size) {
                slice.fill((it & 255) as u8);
            }
            if it % 7 == 0 {
                buf.commit(size);
                if it % 3 == 0 {
                    buf.consume(size);
                }
            }

            if buf.head > buf.tail {
                wrapped += 1;
            }

            it += 1;
        }

        buf.consume(buf.used());
        assert!(buf.used() == 0);
        assert!(buf.free() == buf.size());
        assert!(buf.size() == page_size);
    }

    #[test]
    fn mirrored_buffer_committed() {
        let mut buf = MirroredBuffer::new(1, Some(&next_buffer_index()), Some(0)).unwrap();

        let claimed = buf
            .claim(buf.size())
            .expect("could not claim the entire size");
        claimed.fill(1);

        assert!(buf.commit(buf.size()) == buf.size());
        assert!(buf.used() == buf.size());
        assert!(buf.head == buf.tail);

        let committed = buf.committed().expect("should have something committed");
        assert!(committed.len() == buf.size());

        assert!(buf.size() > 100);
        buf.consume(100);
        let committed = buf.committed().unwrap();
        assert!(committed.len() == buf.size() - 100);
        assert!(buf.head > buf.tail); // wrapped

        let claimed = buf.claim(50).expect("could not claim 50");
        claimed.fill(2);

        assert!(buf.commit(50) == 50);
        assert!(buf.used() == buf.size() - 50);
        assert!(buf.head > buf.tail);

        let committed = buf.committed().unwrap();
        assert!(committed.len() == buf.size() - 50);
        for i in 0..committed.len() - 50 {
            assert!(committed[i] == 1);
        }
        for i in committed.len() - 50..committed.len() {
            assert!(committed[i] == 2);
        }
        assert!(buf.slice.iter().all(|&x| x == 1 || x == 2));
    }
}
