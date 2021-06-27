use core::tasks::*;
use errors::*;
use parking_lot::Mutex;
use std::fmt::Display;
use std::mem;
use std::sync::Arc;
use std::time::{Duration, SystemTime};
use serenity::model::prelude::*;

#[derive(Copy, Clone, Debug)]
struct DeleteRequest(ChannelId, MessageId);
impl AsRef<MessageId> for DeleteRequest {
    fn as_ref(&self) -> &MessageId {
        &self.1
    }
}

struct DeleteServiceData {
    tasks: TaskManager,
    delete_iter_lock: Mutex<()>,
    queued_deletes: Mutex<Vec<DeleteRequest>>,
}

fn check_delete_result<T, E: Display>(id: MessageId, r: StdResult<T, E>) -> Result<()> {
    if let Err(e) = r {
        warn!("Could not delete message {}: {}", id.0, e);
    }
    Ok(())
}
fn check_mass_delete_result<T, E: Display>(
    ids: &[DeleteRequest], r: StdResult<T, E>,
) -> Result<()> {
    if let Err(e) = r {
        let mut msg_temp = Vec::new();
        for id in ids {
            msg_temp.push(id.0);
        }
        warn!("Could not delete messages {:?}: {}", msg_temp, e);
    }
    Ok(())
}

#[derive(Clone)]
pub struct DeleteService(Arc<DeleteServiceData>);
impl DeleteService {
    pub fn new(tasks: TaskManager) -> DeleteService {
        let service = DeleteService(Arc::new(DeleteServiceData {
            tasks: tasks.clone(),
            delete_iter_lock: Mutex::new(()),
            queued_deletes: Mutex::new(Vec::new()),
        }));
        {
            let service = service.clone();
            tasks.dispatch_repeating_task(Duration::from_secs(1), move |_| {
                service.do_queued_deletes();
                Ok(())
            })
        }
        service
    }

    fn do_queued_deletes(&self) {
        if let Some(_lock) = self.0.delete_iter_lock.try_lock() {
            let mut queued = mem::replace(&mut *self.0.queued_deletes.lock(), Vec::new());
            if queued.is_empty() {
                return
            }
            queued.sort_by_key(|x| x.0);

            let mut i = 0;
            let mut current_channel_start = 0;
            let mut current_channel_id = ChannelId(0x3ff001); // dummy channel id
            loop {
                if i == queued.len() || queued[i].0 != current_channel_id {
                    let msgs = &queued[current_channel_start..i];
                    if msgs.len() == 1 {
                        let msg = msgs[0].1;
                        check_delete_result(msg,
                                            current_channel_id.delete_message(msg)).ok();
                    } else if msgs.len() > 1 {
                        check_mass_delete_result(msgs,
                                                 current_channel_id.delete_messages(msgs)).ok();
                    }

                    if i == queued.len() {
                        return
                    } else {
                        current_channel_start = i;
                        current_channel_id = queued[i].0;
                    }
                }
                i += 1;
            }
        }
    }

    pub fn queue_delete_message(&self, msg: &Message) {
        let cutoff = SystemTime::now() - Duration::from_secs(60 * 60 * 24 * 7);
        if cutoff > msg.timestamp.into() {
            let channel_id = msg.channel_id;
            let id = msg.id;
            check_delete_result(id, channel_id.delete_message(id)).ok();
        } else {
            self.0.queued_deletes.lock().push(DeleteRequest(msg.channel_id, msg.id));
            let delete_service = self.clone();
            delete_service.do_queued_deletes();
        }
    }
}