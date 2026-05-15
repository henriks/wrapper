use std::collections::HashMap;
use std::io;
use std::os::unix::io::RawFd;
use std::time::Duration;

use mio::unix::SourceFd;
use mio::{Events, Interest, Poll, Token};
use smoltcp::iface::SocketHandle;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum VmnetEventSource {
    QemuStream,
    HostListener(usize),
    HostSession(SocketHandle),
    UpstreamSession(SocketHandle),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct VmnetReadyEvent {
    pub source: VmnetEventSource,
    pub readable: bool,
    pub writable: bool,
    pub error: bool,
    pub read_closed: bool,
    pub write_closed: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct VmnetInterest {
    readable: bool,
    writable: bool,
}

impl VmnetInterest {
    pub const READABLE: Self = Self {
        readable: true,
        writable: false,
    };
    pub const WRITABLE: Self = Self {
        readable: false,
        writable: true,
    };
    pub const READ_WRITE: Self = Self {
        readable: true,
        writable: true,
    };

    pub fn new(readable: bool, writable: bool) -> Option<Self> {
        (readable || writable).then_some(Self { readable, writable })
    }

    fn mio(self) -> Interest {
        match (self.readable, self.writable) {
            (true, true) => Interest::READABLE | Interest::WRITABLE,
            (true, false) => Interest::READABLE,
            (false, true) => Interest::WRITABLE,
            (false, false) => unreachable!("empty readiness interest is not registered"),
        }
    }
}

#[derive(Debug)]
pub struct RuntimePoller {
    poll: Poll,
    events: Events,
    next_token: usize,
    by_source: HashMap<VmnetEventSource, Registration>,
    by_token: HashMap<Token, VmnetEventSource>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Registration {
    token: Token,
    fd: RawFd,
    interest: VmnetInterest,
}

impl RuntimePoller {
    pub fn new() -> io::Result<Self> {
        Ok(Self {
            poll: Poll::new()?,
            events: Events::with_capacity(256),
            next_token: 0,
            by_source: HashMap::new(),
            by_token: HashMap::new(),
        })
    }

    pub fn register_fd(
        &mut self,
        source: VmnetEventSource,
        fd: RawFd,
        interest: VmnetInterest,
    ) -> io::Result<()> {
        if let Some(existing) = self.by_source.get(&source).copied() {
            if existing.fd == fd {
                if existing.interest != interest {
                    self.reregister_fd(source, fd, interest)?;
                }
                return Ok(());
            }
            self.deregister(source)?;
        }

        let token = Token(self.next_token);
        self.next_token += 1;
        self.poll
            .registry()
            .register(&mut SourceFd(&fd), token, interest.mio())?;
        self.by_source.insert(
            source,
            Registration {
                token,
                fd,
                interest,
            },
        );
        self.by_token.insert(token, source);
        Ok(())
    }

    pub fn is_registered(&self, source: VmnetEventSource) -> bool {
        self.by_source.contains_key(&source)
    }

    pub fn registered_interest(&self, source: VmnetEventSource) -> Option<VmnetInterest> {
        self.by_source
            .get(&source)
            .map(|registration| registration.interest)
    }

    pub fn registered_fd(&self, source: VmnetEventSource) -> Option<RawFd> {
        self.by_source
            .get(&source)
            .map(|registration| registration.fd)
    }

    pub fn registration_count(&self) -> usize {
        self.by_source.len()
    }

    pub fn reregister_fd(
        &mut self,
        source: VmnetEventSource,
        fd: RawFd,
        interest: VmnetInterest,
    ) -> io::Result<()> {
        let Some(existing) = self.by_source.get_mut(&source) else {
            return self.register_fd(source, fd, interest);
        };
        if existing.fd != fd {
            self.deregister(source)?;
            return self.register_fd(source, fd, interest);
        }
        self.poll
            .registry()
            .reregister(&mut SourceFd(&fd), existing.token, interest.mio())?;
        existing.interest = interest;
        Ok(())
    }

    pub fn deregister(&mut self, source: VmnetEventSource) -> io::Result<()> {
        let Some(existing) = self.by_source.remove(&source) else {
            return Ok(());
        };
        self.by_token.remove(&existing.token);
        match self.poll.registry().deregister(&mut SourceFd(&existing.fd)) {
            Ok(()) => Ok(()),
            Err(error)
                if matches!(error.raw_os_error(), Some(libc::EBADF) | Some(libc::ENOENT)) =>
            {
                Ok(())
            }
            Err(error) => Err(error),
        }
    }

    pub fn poll(&mut self, timeout: Option<Duration>) -> io::Result<Vec<VmnetReadyEvent>> {
        self.events.clear();
        self.poll.poll(&mut self.events, timeout)?;
        let mut ready = Vec::new();
        for event in self.events.iter() {
            let Some(source) = self.by_token.get(&event.token()).copied() else {
                continue;
            };
            ready.push(VmnetReadyEvent {
                source,
                readable: event.is_readable(),
                writable: event.is_writable(),
                error: event.is_error(),
                read_closed: event.is_read_closed(),
                write_closed: event.is_write_closed(),
            });
        }
        Ok(ready)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use std::os::unix::io::AsRawFd;
    use std::os::unix::net::UnixStream;

    #[test]
    fn registers_domain_source_without_leaking_mio_tokens() {
        let (_writer, reader) = UnixStream::pair().expect("stream pair");
        reader.set_nonblocking(true).expect("reader nonblocking");

        let mut poller = RuntimePoller::new().expect("poller");
        poller
            .register_fd(
                VmnetEventSource::QemuStream,
                reader.as_raw_fd(),
                VmnetInterest::READABLE,
            )
            .expect("register reader");

        let events = poller
            .poll(Some(Duration::from_millis(0)))
            .expect("poll without blocking");

        assert!(poller.is_registered(VmnetEventSource::QemuStream));
        assert!(events
            .iter()
            .all(|event| event.source == VmnetEventSource::QemuStream));
    }

    #[test]
    fn reregisters_and_deregisters_sources() {
        let (_writer, reader) = UnixStream::pair().expect("stream pair");
        reader.set_nonblocking(true).expect("reader nonblocking");

        let mut poller = RuntimePoller::new().expect("poller");
        poller
            .register_fd(
                VmnetEventSource::HostListener(0),
                reader.as_raw_fd(),
                VmnetInterest::READABLE,
            )
            .expect("register");
        assert_eq!(poller.registration_count(), 1);
        assert_eq!(
            poller.registered_interest(VmnetEventSource::HostListener(0)),
            Some(VmnetInterest::READABLE)
        );
        poller
            .reregister_fd(
                VmnetEventSource::HostListener(0),
                reader.as_raw_fd(),
                VmnetInterest::READ_WRITE,
            )
            .expect("reregister");
        assert_eq!(
            poller.registered_interest(VmnetEventSource::HostListener(0)),
            Some(VmnetInterest::READ_WRITE)
        );
        poller
            .deregister(VmnetEventSource::HostListener(0))
            .expect("deregister");
        assert_eq!(poller.registration_count(), 0);

        let events = poller.poll(Some(Duration::from_millis(1))).expect("poll");
        assert!(events.is_empty());
    }

    #[test]
    fn detects_data_written_before_registration() {
        let (mut writer, reader) = UnixStream::pair().expect("stream pair");
        reader.set_nonblocking(true).expect("reader nonblocking");
        writer.write_all(b"queued-before-register").expect("write");

        let mut poller = RuntimePoller::new().expect("poller");
        poller
            .register_fd(
                VmnetEventSource::QemuStream,
                reader.as_raw_fd(),
                VmnetInterest::READABLE,
            )
            .expect("register reader");

        let events = poller
            .poll(Some(Duration::from_millis(50)))
            .expect("poll ready");

        assert!(events
            .iter()
            .any(|event| { event.source == VmnetEventSource::QemuStream && event.readable }));
    }

    #[test]
    fn replacing_source_fd_ignores_stale_readiness_from_old_fd() {
        let (mut old_writer, old_reader) = UnixStream::pair().expect("old pair");
        let (mut new_writer, new_reader) = UnixStream::pair().expect("new pair");
        old_reader.set_nonblocking(true).expect("old nonblocking");
        new_reader.set_nonblocking(true).expect("new nonblocking");

        let mut poller = RuntimePoller::new().expect("poller");
        poller
            .register_fd(
                VmnetEventSource::HostSession(Default::default()),
                old_reader.as_raw_fd(),
                VmnetInterest::READABLE,
            )
            .expect("register old");
        poller
            .register_fd(
                VmnetEventSource::HostSession(Default::default()),
                new_reader.as_raw_fd(),
                VmnetInterest::READABLE,
            )
            .expect("replace fd");
        assert_eq!(
            poller.registered_fd(VmnetEventSource::HostSession(Default::default())),
            Some(new_reader.as_raw_fd())
        );
        old_writer.write_all(b"old").expect("write old");
        assert!(poller
            .poll(Some(Duration::from_millis(1)))
            .expect("poll old")
            .is_empty());

        new_writer.write_all(b"new").expect("write new");
        let events = poller
            .poll(Some(Duration::from_millis(50)))
            .expect("poll new");
        assert!(events.iter().any(|event| {
            event.source == VmnetEventSource::HostSession(Default::default()) && event.readable
        }));
    }

    #[test]
    fn reregistered_interest_changes_ready_events() {
        let (mut writer, reader) = UnixStream::pair().expect("stream pair");
        reader.set_nonblocking(true).expect("reader nonblocking");
        let mut poller = RuntimePoller::new().expect("poller");
        poller
            .register_fd(
                VmnetEventSource::UpstreamSession(Default::default()),
                reader.as_raw_fd(),
                VmnetInterest::WRITABLE,
            )
            .expect("register writable");
        assert_eq!(
            poller.registered_interest(VmnetEventSource::UpstreamSession(Default::default())),
            Some(VmnetInterest::WRITABLE)
        );
        poller
            .reregister_fd(
                VmnetEventSource::UpstreamSession(Default::default()),
                reader.as_raw_fd(),
                VmnetInterest::READABLE,
            )
            .expect("reregister readable");
        assert_eq!(
            poller.registered_interest(VmnetEventSource::UpstreamSession(Default::default())),
            Some(VmnetInterest::READABLE)
        );

        writer.write_all(b"readable-now").expect("write");
        let events = poller
            .poll(Some(Duration::from_millis(50)))
            .expect("poll readable");

        assert!(events.iter().any(|event| {
            event.source == VmnetEventSource::UpstreamSession(Default::default())
                && event.readable
                && !event.writable
        }));
    }
}
