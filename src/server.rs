use std::collections::HashMap;
use std::sync::Arc;

use tokio::signal;
use tokio::sync::mpsc;
use tokio::sync::RwLock;
use tokio_stream::wrappers::ReceiverStream;
use tonic::{Request, Response, Status};
use tonic::transport::Server;
use uuid::{Uuid, adapter::Simple};

use chat::{
    chat_server::{Chat, ChatServer},
    Admittance,
    ChatMessage,
    CommitAck,
    Member,
    NewChatMessage,
    TimeSpan
};

pub mod chat {
    tonic::include_proto!("chat");
}

#[tonic::async_trait]
trait BasicAuthentication {
    type Token;
    type User;
    async fn is_registered(&self, member: &Self::User) -> bool;
    async fn is_authenticated(&self, token: &Self::Token) -> bool;
    async fn authenticate(&self, member: &Self::User) -> Option<Self::Token>;
    async fn user(&self, token: &Self::Token) -> Option<Self::User>;
}

const DEFAULT_HISTORY_DEPTH: usize = 20;

#[derive(Debug)]
pub struct ChatService {
    chat_history: Arc<RwLock<Vec<ChatMessage>>>,
    access_tokens: Arc<RwLock<HashMap<Simple, Member>>>,
    registered_members: Arc<RwLock<HashMap<String, Member>>>,
}

#[tonic::async_trait]
impl BasicAuthentication for ChatService {
    type Token = String;
    type User = Member;
    async fn is_registered(&self, member: &Self::User) -> bool {
        let members = self.registered_members.read().await;
        members.contains_key(&member.username)
    }

    async fn is_authenticated(&self, token: &Self::Token) -> bool {
        if let Ok(uuid_token) = Uuid::parse_str(token.as_str()) {
            let tokens = self.access_tokens.read().await;
            tokens.contains_key(&uuid_token.to_simple())
        } else {
            false
        }
    }

    async fn authenticate(&self, member: &Self::User) -> Option<Self::Token> {
        if self.is_registered(member).await {
            let token = Uuid::new_v4().to_simple();
            self.access_tokens.write().await
                .insert(token, member.clone());
            Some(token.to_string())
        } else {
            None
        }
    }

    async fn user(&self, token: &Self::Token) -> Option<Self::User> {
        if let Ok(uuid_token) = Uuid::parse_str(token.as_str()) {
            let tokens = self.access_tokens.read().await;
            if let Some(member) = tokens.get(&uuid_token.to_simple()) {
                Some(member.clone())
            } else {
                None
            }
        } else {
            None
        }
    }
}

#[tonic::async_trait]
impl Chat for ChatService {
    async fn join(
        &self,
        request: Request<Member>,
    ) -> Result<Response<Admittance>, Status> {
        log::debug!("Received a new join request.");
        if let Some(token) = self.authenticate(request.get_ref()).await {
            let admit = Admittance {
                token
            };
            log::debug!("A token is generated for {}.", request.get_ref().username);

            Ok(Response::new(admit))
        } else {
            log::error!("Invalid credentials");
            Err(Status::unauthenticated("Can not authenticate with given credentials."))
        }
    }

    type ChatLogStream = ReceiverStream<Result<ChatMessage, Status>>;

    async fn chat_log(
        &self,
        request: Request<TimeSpan>,
    ) -> Result<Response<Self::ChatLogStream>, Status> {
        if !self.is_authenticated(&request.get_ref().access_token).await {
            return Err(Status::unauthenticated("Invalid Access-Token"));
        }

        let (tx, rx) = mpsc::channel(100);
        let history_ref = self.chat_history.clone();

        tokio::spawn(async move {
            let timespan = request.get_ref();
            let asked_start = timespan.start as usize;
            let asked_end = timespan.end as usize; // If timespanend == -1, it'll map to biggest usize
            let history = history_ref.read().await;
            let history_len = history.len();

            log::debug!("Asking for messages between {} and {}", asked_start, asked_end);

            let (start, end) = {
                if asked_start < history_len {
                    if asked_end > asked_start {
                        if asked_end <= history_len {
                            (asked_start, asked_end)
                        } else {
                            (asked_start, history_len)
                        }
                    } else {
                        (asked_start, history_len)
                    }
                } else if asked_start == history_len {
                    (history_len, history_len)
                } else {
                    (history_len - DEFAULT_HISTORY_DEPTH, history_len)
                }
            };

            log::debug!("Sending messages between start {} end {}", start, end);

            for chat_msg in &history[start..end] {
                tx.send(Ok(chat_msg.clone())).await.unwrap();
            }

        });

        Ok(Response::new(ReceiverStream::new(rx)))
    }

    async fn commit(
        &self,
        request: Request<NewChatMessage>,
    ) -> Result<Response<CommitAck>, Status> {
        if let Some(member) = self.user(&request.get_ref().access_token).await {
            let mut chat_log = self.chat_history.write().await;
            let time = chat_log.len() as u64;

            log::debug!(
                "Message committed to {}: {} says '{}'",
                time,
                member.username,
                request.get_ref().value
            );

            chat_log.push(ChatMessage {
                time,
                username: member.username,
                value: request.get_ref().value.clone(),
            });


            Ok(Response::new(CommitAck{ time }))
        } else {
            log::debug!("Authentication failed for {}", request.get_ref().access_token);
            Err(Status::unauthenticated("Invalid Access-Token"))
        }
    }
}

async fn cleanup(){
    signal::ctrl_c().await.unwrap();
    log::info!("Good bye!");
}


#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    env_logger::Builder::new()
        .filter_module(
            "server", log::LevelFilter::Debug
        ).init();

    let addr = "[::1]:10000".parse()?;

    let mut users = HashMap::new();
    users.insert("osman".to_string(), Member{
        username: "osman".to_string(),
        password: "1234".to_string(),
    });
    users.insert("orhan".to_string(), Member{
        username: "orhan".to_string(),
        password: "1234".to_string(),
    });

    let sample_messages = vec![
        ChatMessage {
            time: 0,
            username: "osman".to_string(),
            value: "Hi, how is it going?".to_string(),
        },
        ChatMessage {
            time: 1,
            username: "orhan".to_string(),
            value: "not much. how are things with you?".to_string(),
        },
        ChatMessage {
            time: 2,
            username: "osman".to_string(),
            value: "good!".to_string(),
        },
    ];

    let chat_service = ChatService {
        chat_history: Arc::new(
            RwLock::new(
                sample_messages
            )
        ),
        access_tokens: Arc::new(
            RwLock::new(
                HashMap::new()
            )
        ),
        registered_members: Arc::new(
            RwLock::new(
                users
            )
        )
    };

    let service = ChatServer::new(chat_service);

    Server::builder()
        .add_service(service)
        .serve_with_shutdown(addr, cleanup())
        .await?;

    Ok(())
}
