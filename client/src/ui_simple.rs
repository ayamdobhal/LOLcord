use iced::widget::{button, column, container, text, text_input};
use iced::{Alignment, Element, Length, Task, Theme};

#[derive(Debug, Clone)]
pub enum Message {
    ServerAddrChanged(String),
    ConnectPressed,
}

pub struct App {
    server_addr: String,
}

impl iced::Application for App {
    type Executor = iced::executor::Default;
    type Message = Message;
    type Theme = Theme;
    type Flags = ();

    fn new(_flags: Self::Flags) -> (Self, Task<Self::Message>) {
        (
            App {
                server_addr: "127.0.0.1:3000".to_string(),
            },
            Task::none(),
        )
    }

    fn title(&self) -> String {
        "LOLcord".to_string()
    }

    fn update(&mut self, message: Self::Message) -> Task<Self::Message> {
        match message {
            Message::ServerAddrChanged(addr) => {
                self.server_addr = addr;
            }
            Message::ConnectPressed => {
                println!("Connecting to {}", self.server_addr);
            }
        }
        Task::none()
    }

    fn view(&self) -> Element<Self::Message> {
        let content = column![
            text("LOLcord").size(24),
            text_input("Server address", &self.server_addr)
                .on_input(Message::ServerAddrChanged)
                .padding(10),
            button("Connect").on_press(Message::ConnectPressed).padding(10),
        ]
        .spacing(20)
        .align_items(Alignment::Center);

        container(content)
            .width(Length::Fill)
            .height(Length::Fill)
            .center_x()
            .center_y()
            .into()
    }

    fn theme(&self) -> Self::Theme {
        Theme::Dark
    }
}