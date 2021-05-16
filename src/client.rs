use std::time::Duration;
use std::sync::Arc;
use std::sync::mpsc as std_mpsc;

use console::{Key, Term};
use tokio::time;
use tonic::Request;
use tokio::sync::RwLock;
use tokio::sync::mpsc;
use tonic::transport::Channel;

use chat::chat_client::ChatClient;
use chat::{
    ChatMessage,
    CommitAck,
    Member,
    NewChatMessage,
    TimeSpan,
};

pub mod chat {
    tonic::include_proto!("chat");
}

const MSG_CHECK_INTERVAL: Duration = Duration::from_millis(500);

#[derive(Clone)]
struct ClientState {
    chat_log: Arc<RwLock<Vec<ChatMessage>>>,
}

struct Client {
    access_token: String,
    last_msg_idx: u64,
    new_msg_recv: mpsc::Receiver<String>,
    _client: ChatClient<Channel>,
    _state: ClientState,
}

impl Client {
    async fn new(
        client_state: ClientState,
        new_msg_recv: mpsc::Receiver<String>,
        server_addr: &'static str
    ) -> Result<Self, Box<dyn std::error::Error>> {
        let _client = ChatClient::connect(server_addr).await?;
        println!("Connected to {}", server_addr);
        Ok(Client {
            access_token: String::default(),
            last_msg_idx: u64::default(),
            new_msg_recv,
            _client,
            _state: client_state,
        })
    }

    async fn obtain_token(&mut self) -> Result<&mut Self, tonic::Status> {
        let username: String = dialoguer::Input::new()
            .with_prompt("Username")
            .interact_text()?;
        let password: String = dialoguer::Password::new()
            .with_prompt("Password")
            .interact()?;

        let me = Member {
            username,
            password,
        };

        let response = self._client.join(Request::new(me)).await?;
        println!("Logged in!");

        self.access_token = response.get_ref().token.to_owned();

        Ok(self)
    }

    async fn retrieve_messages(&mut self) -> Result<&mut Self, Box<dyn std::error::Error>> {
        let time_span = TimeSpan {
            start: self.last_msg_idx + 1,
            access_token: self.access_token.clone(),
            end: -1,
        };

        let mut stream = self._client
            .chat_log(Request::new(time_span))
            .await?
            .into_inner();

        while let Some(msg) = stream.message().await? {
            self.last_msg_idx = msg.time;
            self._state.chat_log.write().await.push(msg);
        }

        Ok(self)
    }

    async fn send_message(&mut self, msg: String) -> Result<CommitAck, Box<dyn std::error::Error>> {
        let new_msg = NewChatMessage {
            access_token: self.access_token.clone(),
            value: msg,
        };

        let ack = self._client.commit(Request::new(new_msg)).await?;

        Ok(ack.get_ref().to_owned())
    }

    async fn user_message(&mut self) -> Result<(), Box<dyn std::error::Error>> {
        if let Some(msg) = self.new_msg_recv.recv().await {
            self.send_message(msg).await?;
        }
        Ok(())
    }

    async fn latch(&mut self) -> Result<(), Box<dyn std::error::Error>> {
        let mut instant = time::interval(MSG_CHECK_INTERVAL);
        loop {
            instant.tick().await;
            self.retrieve_messages().await?;
            self.user_message().await?;
        }
    }
}

struct ChatWindow {
    _buffer: Arc<RwLock<Vec<ChatMessage>>>,
    _term: console::Term,
    _state: ClientState,
    _current_msg: String,
}

impl ChatWindow {
    fn new(state: ClientState) -> Self {
        ChatWindow {
            _term: Term::stdout(),
            _buffer: Arc::new(RwLock::new(Vec::new())),
            _state: state,
            _current_msg: String::new(),
        }
    }

    async fn play(
        &mut self,
        msg_tx: mpsc::Sender<String>
    ) -> Result<(), Box<dyn std::error::Error>> {
        let mut instant = time::interval(Duration::from_millis(1));

        let (key_tx, key_rx) = std_mpsc::channel::<Key>();

        let _input_task = tokio::task::spawn(async move {
            let input_term = Term::stdout();
            loop {
                if let Ok(key) = input_term.read_key() {
                    if key_tx.send(key).is_err() {
                        break;
                    }
                }
            }
        });

        self._term.hide_cursor()?;


        loop {
            instant.tick().await;

            self._term.clear_screen()?;

            self._term.write_line(" Chat Program 3000")?;
            self._term.write_line("------------------------------")?;

            let msg_log = self._state.chat_log.read().await;
            for msg in msg_log.iter() {
                let m = format!("{}: {}", msg.username, msg.value);
                self._term.write_line(m.as_str())?;
            }

            loop {
                match key_rx.try_recv() {
                    Ok(key) => {
                        match key {
                            Key::Char(x) => self._current_msg.push(x),
                            Key::Enter => {
                                msg_tx.send(self._current_msg.clone()).await?;
                                self._current_msg = String::new();
                            }
                            _ => (),
                        }
                    },
                    Err(_) => break,
                }
            }

            let current_msg_prompt = format!("> {}", self._current_msg);
            self._term.write_line(current_msg_prompt.as_str())?;

            self._term.flush()?;
        }
    }
}


#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>>{

    let server_addr = "http://[::1]:10000";

    let state = ClientState {
        chat_log: Arc::new(RwLock::new(Vec::new())),
    };

    let (msg_tx, msg_rx) = mpsc::channel(10);

    if let Ok(mut client) = Client::new(state.clone(), msg_rx, server_addr).await {
        match client.obtain_token().await {
            Err(e) => {
                println!("Error logging in: {:?}", e.message());
                return Ok(());
            },
            _ => ()
        }

        let client_task = tokio::spawn(async move {
            if let Err(_) = client.latch().await {
                println!("End of messages");
            }
        });

        if let Err(_) = ChatWindow::new(state.clone()).play(msg_tx).await {
            println!("Closing chat window");
        };
        if let Err(_) = client_task.await {
            println!("Ending client")
        };
    }

    println!("Good bye!");

    Ok(())
}
