mod error;
mod util;

pub use error::{Error, ErrorKind};
use std::{cmp, ffi::CString, io, process};
use util::round_up_to_page_size;

// TODO example usage with UDS + a frame and a streaming codec

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
    pub fn new(
        size: usize,
        name_suffix: Option<&str>,
        initial_value: Option<u8>,
    ) -> Result<MirroredBuffer<'a>, Error> {
        if size == 0 {
            return Err(Error::invalid_size(size));
        }

        let name;
        if let Some(suffix) = name_suffix {
            name = format!("/mirrored-buffer-{}-{}", process::id(), suffix);
        } else {
            name = format!("/mirrored-buffer-{}", process::id());
        }

        let name = CString::new(name.as_str()).unwrap_or_else(|_| {
            panic!(
                "invalid name: {} - contains a 0-byte when it should not",
                name,
            )
        });

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
            return Err(Error::invalid_size(size_total));
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
        Some(&mut self.slice[self.tail..(self.tail + size)])
    }

    pub fn commit(&mut self, mut size: usize) -> usize {
        size = cmp::min(size, self.free());
        self.size_used += size;
        self.tail = (self.tail + size) & self.size_mask;
        size
    }

    pub fn consume(&mut self, mut size: usize) -> usize {
        size = cmp::min(size, self.used());
        self.size_used -= size;
        self.head = (self.head + size) & self.size_mask;
        size
    }

    pub fn committed(&self) -> Option<&[u8]> {
        if self.used() == 0 {
            return None;
        }

        if self.head < self.tail {
            return Some(&self.slice[self.head..self.tail]);
        }
        Some(&self.slice[self.head..(self.tail + self.size())])
    }
}

impl<'a> Drop for MirroredBuffer<'a> {
    fn drop(&mut self) {
        println!("dropped");
        if unsafe { libc::shm_unlink(self.name.as_ptr()) } != 0 {
            panic!("{}", io::Error::last_os_error());
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::{util::get_page_size, MirroredBuffer};

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
        index.to_string()
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
    fn mirrored_buffer_is_mirrored() {
        let mut buf = MirroredBuffer::new(
            get_page_size().unwrap(),
            Some(&next_buffer_index()),
            Some(0),
        )
        .unwrap();

        let size = buf.size();
        let claim_size = size / 2;
        assert!(claim_size > 0);

        let claimed = buf.claim(claim_size).unwrap();
        assert!(claimed.iter().all(|&x| x == 0));
        claimed.fill(8);
        assert!(buf.head == 0);
        assert!(buf.tail == 0);

        buf.commit(claim_size);
        assert!(buf.head == 0);
        assert!(buf.tail == claim_size);

        // We wrote 8 in the first half of [0..size] which is mirrored in
        // [size..size * 2] - as such, the latter slice should also have 8 in
        // its first half.
        assert!(&buf.slice[size..size + claim_size].iter().all(|&x| x == 8));
        assert!(&buf.slice[size + claim_size..size * 2]
            .iter()
            .all(|&x| x == 0));

        // We write 13 in the second half of [size..size * 2], expecting it to
        // get mirrored in the first half of [0..size].
        let writein = &mut buf.slice[size + claim_size..size * 2];
        writein.fill(13);

        assert!(&buf.slice[claim_size..size].iter().all(|&x| x == 13));

        // sanity check
        let committed = buf.committed().unwrap();
        assert!(committed[..claim_size].iter().all(|&x| x == 8));
        assert!(committed[claim_size..].iter().all(|&x| x == 13));
    }

    #[test]
    fn mirrored_buffer_claim_commit_consume() {
        let page_size = get_page_size().unwrap();
        let mut buf = MirroredBuffer::new(page_size, Some(&next_buffer_index()), Some(0)).unwrap();

        assert!(buf.claim(0).is_none());

        // claim, head and tail do not change
        let claim_size = page_size / 2;
        assert!(claim_size > 0);
        let claimed = buf.claim(claim_size);
        assert!(claimed.is_some());
        let claimed = claimed.unwrap();
        assert!(claimed.iter().all(|&x| x == 0));
        claimed.fill(1);

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
        let claimed = buf.claim(page_size);
        assert!(claimed.is_some());
        let claimed = claimed.unwrap();
        claimed.fill(2);

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
        for x in committed[..committed.len() - 50].iter() {
            assert!(*x == 1);
        }
        for x in committed[committed.len() - 50..].iter() {
            assert!(*x == 2);
        }
        assert!(buf.slice.iter().all(|&x| x == 1 || x == 2));
    }
}
