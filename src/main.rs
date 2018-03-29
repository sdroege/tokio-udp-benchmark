// Testcase/benchmark for https://github.com/tokio-rs/tokio/issues/265

extern crate futures;
extern crate tokio;
extern crate tokio_executor;
extern crate tokio_reactor;

use futures::{future, stream};
use futures::{Async, Future, Stream};
use futures::stream::futures_unordered::FuturesUnordered;
use tokio::executor::thread_pool;
use tokio::reactor;

use std::{ops, thread, time};
use std::sync::{Arc, Mutex};

const N_THREADS: isize = -1;

const PORTS: ops::Range<u16> = 60000..60200;

fn main() {
    // Start our thread that just sends packets forever to the ports
    thread::spawn(|| {
        use std::net;
        use std::net::{IpAddr, Ipv4Addr, SocketAddr};

        let buffer = [0; 160];
        let socket = net::UdpSocket::bind("0.0.0.0:50000").unwrap();

        let ipaddr = IpAddr::V4(Ipv4Addr::new(127, 0, 0, 1));
        let destinations = PORTS
            .map(|port| SocketAddr::new(ipaddr, port))
            .collect::<Vec<_>>();

        thread::sleep(time::Duration::from_millis(1000));

        loop {
            for dest in &destinations {
                socket.send_to(&buffer, dest).unwrap();
            }
            thread::sleep(time::Duration::from_millis(10));
        }
    });

    // Future that receives from all the ports and bounces packets back
    let future = future::lazy(|| {
        use tokio::net;
        use std::net::{IpAddr, Ipv4Addr, SocketAddr};

        let mut futures = FuturesUnordered::new();

        let ipaddr = IpAddr::V4(Ipv4Addr::new(0, 0, 0, 0));
        for port in PORTS {
            let saddr = SocketAddr::new(ipaddr, port);
            let socket = Arc::new(Mutex::new(net::UdpSocket::bind(&saddr).unwrap()));
            let socket_clone = socket.clone();

            let future = stream::poll_fn(move || {
                let mut buffer = vec![0; 1500];

                match socket.lock().unwrap().poll_recv_from(buffer.as_mut_slice()) {
                    Ok(Async::NotReady) => Ok(Async::NotReady),
                    Err(_) => {
                        unimplemented!();
                    }
                    Ok(Async::Ready((len, addr))) => {
                        buffer.truncate(len);
                        Ok(Async::Ready(Some((buffer, addr))))
                    }
                }
            }).for_each(move |(buffer, addr)| {
                let socket = socket_clone.clone();

                future::poll_fn(move || {
                    match socket
                        .lock()
                        .unwrap()
                        .poll_send_to(buffer.as_slice(), &addr)
                    {
                        Ok(Async::NotReady) => Ok(Async::NotReady),
                        Err(_) => {
                            unimplemented!();
                            // just for type inference
                            Err(())
                        }
                        Ok(Async::Ready(len)) => {
                            assert_eq!(len, buffer.len());
                            Ok(Async::Ready(()))
                        }
                    }
                })
            });

            futures.push(future);
        }

        futures.for_each(|_| Ok(()))
    });

    let reactor = reactor::Reactor::new().unwrap();
    if N_THREADS >= 0 {
        // Thread-pool based executor, basically the standard Runtime
        let mut pool_builder = thread_pool::Builder::new();
        let handle = reactor.handle();
        pool_builder.around_worker(move |w, enter| {
            ::tokio_reactor::with_default(&handle, enter, |_| {
                w.run();
            });
        });

        if N_THREADS > 0 {
            pool_builder.pool_size(N_THREADS as usize);
        }

        let pool = pool_builder.build();
        pool.spawn(future);

        let _bg = reactor.background();

        pool.shutdown().wait().unwrap();
    } else {
        // Single-threaded Reactor/Executor
        use tokio::executor::current_thread;

        reactor.set_fallback().unwrap();
        let handle = reactor.handle();
        let mut enter = ::tokio_executor::enter().unwrap();
        let mut current_thread = current_thread::CurrentThread::new_with_park(reactor);

        current_thread.spawn(future);

        ::tokio_reactor::with_default(&handle, &mut enter, move |enter| {
            current_thread.enter(enter).run().unwrap();
        });
    }
}
