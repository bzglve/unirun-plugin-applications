mod app_info;

use std::{cell::RefCell, pin::Pin, rc::Rc};

use app_info::AppInfo;
use gio::prelude::*;
use glib::{self, clone};
#[allow(unused_imports)]
use log::*;
use unirun_if::{
    package::{Command, Hit, Package, PackageId, Payload},
    socket::{connection, stream_read_future, stream_write_future},
};

fn handle_get_data(
    hits: Vec<Hit>,
    pack_id: PackageId,
    connection: gio::SocketConnection,
    main_loop: glib::MainLoop,
) -> Pin<Box<dyn std::future::Future<Output = ()>>> {
    Box::pin(async move {
        let pack = Package::new(Payload::Result(Ok(pack_id)));
        debug!("Sending {:?}", pack);
        stream_write_future(&connection.output_stream(), pack)
            .await
            .unwrap();

        let mut i = 0;
        while i < hits.len() {
            let h = hits.get(i).unwrap();
            let pack = Package::new(Payload::Hit(h.clone()));
            let pack_id = pack.get_id();

            debug!("Sending {}", h);
            stream_write_future(&connection.output_stream(), pack)
                .await
                .unwrap();

            let response = stream_read_future(&connection.input_stream()).await;

            if response.is_err() {
                main_loop.quit();
            }
            let response = response.unwrap();

            debug!("Got response: {:?}", response);

            match response.payload {
                Payload::Command(Command::Abort) => {
                    // FIXME workaround
                    warn!("ABORTING");
                    connection.output_stream().clear_pending();
                    return;
                }
                Payload::Result(Err(response_id)) => {
                    if response_id == pack_id {
                        continue;
                    }
                }
                Payload::Result(Ok(_)) => {}
                _ => unreachable!(),
            };

            i += 1;
        }

        stream_write_future(
            &connection.output_stream(),
            Package::new(Payload::Command(Command::Abort)),
        )
        .await
        .unwrap();
    })
}

async fn handle_command(
    command: &Command,
    pack_id: PackageId,
    hits: Rc<RefCell<Vec<(Hit, AppInfo)>>>,
    connection: gio::SocketConnection,
    main_loop: glib::MainLoop,
) {
    match command {
        Command::GetData(text) => {
            *hits.borrow_mut() = (if text.is_empty() {
                AppInfo::all()
            } else {
                AppInfo::search(&text)
            })
            .into_iter()
            .map(|app_info| (Hit::from(&app_info), app_info))
            .collect();

            let hits_left = hits.borrow().clone().into_iter().map(|(h, _)| h).collect();
            handle_get_data(hits_left, pack_id, connection, main_loop).await;
        }
        Command::Activate(id) => {
            if let Some(app_info) =
                hits.borrow()
                    .iter()
                    .find_map(|(h, a)| if h.id == *id { Some(a) } else { None })
            {
                stream_write_future(
                    &connection.output_stream(),
                    Package::new(Payload::Result(
                        match app_info.inner.launch(&[], gio::AppLaunchContext::NONE) {
                            Ok(_) => Ok(pack_id),
                            Err(_) => Err(pack_id),
                        },
                    )),
                )
                .await
                .unwrap();
            }
        }
        Command::Abort => {}
        _ => unreachable!(),
    }
}

fn main() -> Result<(), glib::Error> {
    env_logger::init();

    let hits = Rc::new(RefCell::new(Vec::new()));
    let main_loop = glib::MainLoop::new(None, true);
    let conn = connection()?;

    glib::spawn_future_local(clone!(
        #[strong]
        main_loop,
        async move {
            loop {
                debug!("Waiting for command");

                let data = match stream_read_future(&conn.input_stream()).await {
                    Ok(d) => d,
                    Err(_) => {
                        // error!("Failed to read data: {}", e);
                        main_loop.quit();
                        continue;
                    }
                };
                debug!("Received: {:?}", data);

                match &data.payload {
                    Payload::Command(command) => {
                        handle_command(
                            command,
                            data.get_id(),
                            hits.clone(),
                            conn.clone(),
                            main_loop.clone(),
                        )
                        .await
                    }
                    _ => unreachable!(),
                }
            }
        }
    ));

    main_loop.run();
    Ok(())
}
