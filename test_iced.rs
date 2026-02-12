// Test file to check Iced API
use iced::{Application, Element, Task, Theme};

#[derive(Debug, Clone)]
enum Message {}

struct TestApp;

impl Application for TestApp {
    type Executor = iced::executor::Default;
    type Message = Message;
    type Theme = Theme;
    type Flags = ();

    fn new(_flags: Self::Flags) -> (Self, Task<Self::Message>) {
        (TestApp, Task::none())
    }

    fn title(&self) -> String {
        "Test".to_string()
    }

    fn update(&mut self, _message: Self::Message) -> Task<Self::Message> {
        Task::none()
    }

    fn view(&self) -> Element<Self::Message> {
        iced::widget::text("Hello").into()
    }
}

fn main() -> iced::Result {
    TestApp::run(iced::Settings::default())
}