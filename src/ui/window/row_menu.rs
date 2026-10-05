//! A right-click menu for palette rows with one level of fly-out submenus.
//!
//! The menu opens at the cursor (flipped at the window edges); an open fly-out
//! is laid out beside it — on the right when it fits, otherwise on the left —
//! so the menu itself never moves. Pressing a submenu entry keeps the menu open.

use iced::advanced::layout::{self, Layout};
use iced::advanced::widget::{self, tree, Tree, Widget};
use iced::advanced::{mouse, overlay, renderer, Shell};
use iced::{keyboard, Element, Event, Length, Point, Rectangle, Renderer, Size, Theme, Vector};

use crate::app::Message;

const FLYOUT_GAP: f32 = 2.0;

pub(crate) struct RowMenu<'a> {
    underlay: Element<'a, Message>,
    main: Element<'a, Message>,
    /// Per entry of the menu's column: a submenu entry (pressing it keeps the menu open).
    keep_open: Vec<bool>,
    /// The open fly-out: its entry's index in the column, and its content.
    flyout: Option<(usize, Element<'a, Message>)>,
    /// Published when the menu closes with a fly-out open.
    on_close: Message,
}

#[derive(Default)]
struct State {
    show: bool,
    position: Point,
}

impl<'a> RowMenu<'a> {
    pub(crate) fn new(
        underlay: impl Into<Element<'a, Message>>,
        main: Element<'a, Message>,
        keep_open: Vec<bool>,
        flyout: Option<(usize, Element<'a, Message>)>,
        on_close: Message,
    ) -> Self {
        Self { underlay: underlay.into(), main, keep_open, flyout, on_close }
    }
}

impl<'a> Widget<Message, Theme, Renderer> for RowMenu<'a> {
    fn size(&self) -> Size<Length> {
        self.underlay.as_widget().size()
    }

    fn layout(&mut self, tree: &mut Tree, renderer: &Renderer, limits: &layout::Limits) -> layout::Node {
        self.underlay.as_widget_mut().layout(&mut tree.children[0], renderer, limits)
    }

    fn draw(
        &self,
        tree: &Tree,
        renderer: &mut Renderer,
        theme: &Theme,
        style: &renderer::Style,
        layout: Layout<'_>,
        cursor: mouse::Cursor,
        viewport: &Rectangle,
    ) {
        self.underlay.as_widget().draw(&tree.children[0], renderer, theme, style, layout, cursor, viewport);
    }

    fn tag(&self) -> tree::Tag {
        tree::Tag::of::<State>()
    }

    fn state(&self) -> tree::State {
        tree::State::new(State::default())
    }

    fn diff(&mut self, tree: &mut Tree) {
        let mut refs: Vec<&mut Element<'a, Message>> = vec![&mut self.underlay, &mut self.main];
        if let Some((_, f)) = &mut self.flyout {
            refs.push(f);
        }
        tree.diff_children(&mut refs);
    }

    fn update(
        &mut self,
        tree: &mut Tree,
        event: &Event,
        layout: Layout<'_>,
        cursor: mouse::Cursor,
        renderer: &Renderer,
        shell: &mut Shell<'_, Message>,
        viewport: &Rectangle,
    ) {
        if let Event::Mouse(mouse::Event::ButtonPressed(mouse::Button::Right)) = event {
            if let Some(p) = cursor.position_over(layout.bounds()) {
                let state: &mut State = tree.state.downcast_mut();
                state.show = true;
                state.position = p;
                if self.flyout.is_some() {
                    shell.publish(self.on_close.clone());
                }
                shell.capture_event();
                shell.request_redraw();
                return;
            }
        }
        self.underlay
            .as_widget_mut()
            .update(&mut tree.children[0], event, layout, cursor, renderer, shell, viewport);
    }

    fn mouse_interaction(
        &self,
        tree: &Tree,
        layout: Layout<'_>,
        cursor: mouse::Cursor,
        viewport: &Rectangle,
        renderer: &Renderer,
    ) -> mouse::Interaction {
        self.underlay.as_widget().mouse_interaction(&tree.children[0], layout, cursor, viewport, renderer)
    }

    fn operate(
        &mut self,
        tree: &mut Tree,
        layout: Layout<'_>,
        renderer: &Renderer,
        operation: &mut dyn widget::Operation,
    ) {
        self.underlay.as_widget_mut().operate(&mut tree.children[0], layout, renderer, operation);
    }

    fn overlay<'b>(
        &'b mut self,
        tree: &'b mut Tree,
        layout: Layout<'b>,
        renderer: &Renderer,
        viewport: &Rectangle,
        translation: Vector,
    ) -> Option<overlay::Element<'b, Message, Theme, Renderer>> {
        let state: &mut State = tree.state.downcast_mut();
        if !state.show {
            return self.underlay.as_widget_mut().overlay(&mut tree.children[0], layout, renderer, viewport, translation);
        }
        let position = state.position + translation;
        let (_, trees) = tree.children.split_at_mut(1);
        let (main_tree, fly_tree) = trees.split_at_mut(1);
        Some(overlay::Element::new(Box::new(MenuOverlay {
            state,
            position,
            main: &mut self.main,
            main_tree: &mut main_tree[0],
            keep_open: &self.keep_open,
            flyout: self.flyout.as_mut().map(|(i, f)| (*i, f)).zip(fly_tree.first_mut()),
            on_close: &self.on_close,
        })))
    }
}

impl<'a> From<RowMenu<'a>> for Element<'a, Message> {
    fn from(w: RowMenu<'a>) -> Self {
        Element::new(w)
    }
}

struct MenuOverlay<'a, 'b> {
    state: &'b mut State,
    position: Point,
    main: &'b mut Element<'a, Message>,
    main_tree: &'b mut Tree,
    keep_open: &'b [bool],
    flyout: Option<((usize, &'b mut Element<'a, Message>), &'b mut Tree)>,
    on_close: &'b Message,
}

/// The entries of a menu box (container › column › entries).
fn entries(main: Layout<'_>) -> impl Iterator<Item = Layout<'_>> {
    main.children().next().into_iter().flat_map(|column| column.children())
}

impl MenuOverlay<'_, '_> {
    fn close(&mut self, shell: &mut Shell<'_, Message>) {
        self.state.show = false;
        if self.flyout.is_some() {
            shell.publish(self.on_close.clone());
        }
        shell.request_redraw();
    }
}

impl overlay::Overlay<Message, Theme, Renderer> for MenuOverlay<'_, '_> {
    fn layout(&mut self, renderer: &Renderer, bounds: Size) -> layout::Node {
        let limits = layout::Limits::new(Size::ZERO, bounds);
        let main = self.main.as_widget_mut().layout(self.main_tree, renderer, &limits);
        let size = main.size();
        // At the cursor, flipped to the other side at the window edges.
        let mut p = self.position;
        if p.x + size.width > bounds.width {
            p.x = (p.x - size.width).max(0.0);
        }
        if p.y + size.height > bounds.height {
            p.y = (p.y - size.height).max(0.0);
        }
        let main = main.move_to(p);
        let mut nodes = Vec::with_capacity(2);
        if let Some(((index, flyout), tree)) = self.flyout.as_mut() {
            let node = flyout.as_widget_mut().layout(tree, renderer, &limits);
            let fly = node.size();
            // Its first item level with the entry (both boxes have the same inset).
            let entry_y = main.children().first().and_then(|column| column.children().get(*index)).map_or(0.0, |e| e.bounds().y);
            let y = (p.y + entry_y).min(bounds.height - fly.height).max(0.0);
            // On the right when it fits, otherwise on the left.
            let right = p.x + size.width + FLYOUT_GAP;
            let x = if right + fly.width <= bounds.width { right } else { (p.x - fly.width - FLYOUT_GAP).max(0.0) };
            nodes.push(main);
            nodes.push(node.move_to(Point::new(x, y)));
        } else {
            nodes.push(main);
        }
        layout::Node::with_children(bounds, nodes)
    }

    fn draw(&self, renderer: &mut Renderer, theme: &Theme, style: &renderer::Style, layout: Layout<'_>, cursor: mouse::Cursor) {
        let mut children = layout.children();
        if let Some(main) = children.next() {
            self.main.as_widget().draw(self.main_tree, renderer, theme, style, main, cursor, &layout.bounds());
        }
        if let (Some(((_, flyout), tree)), Some(fly)) = (self.flyout.as_ref(), children.next()) {
            flyout.as_widget().draw(tree, renderer, theme, style, fly, cursor, &layout.bounds());
        }
    }

    fn update(
        &mut self,
        event: &Event,
        layout: Layout<'_>,
        cursor: mouse::Cursor,
        renderer: &Renderer,
        shell: &mut Shell<'_, Message>,
    ) {
        let mut children = layout.children();
        let Some(main) = children.next() else { return };
        let fly = children.next();
        let over = cursor.is_over(main.bounds()) || fly.is_some_and(|f| cursor.is_over(f.bounds()));
        match event {
            Event::Keyboard(keyboard::Event::KeyPressed { key: keyboard::Key::Named(keyboard::key::Named::Escape), .. }) => {
                self.close(shell);
                shell.capture_event();
                return;
            }
            Event::Mouse(mouse::Event::ButtonPressed(_)) if !over => {
                self.close(shell);
                return;
            }
            _ => {}
        }
        let viewport = layout.bounds();
        self.main.as_widget_mut().update(self.main_tree, event, main, cursor, renderer, shell, &viewport);
        if let (Some(((_, flyout), tree)), Some(fly)) = (self.flyout.as_mut(), fly) {
            flyout.as_widget_mut().update(tree, event, fly, cursor, renderer, shell, &viewport);
        }
        if let Event::Mouse(mouse::Event::ButtonReleased(mouse::Button::Left)) = event {
            // A chosen entry closes the menu; a submenu entry opens its fly-out instead.
            let on_submenu = entries(main)
                .zip(self.keep_open.iter())
                .any(|(e, keep)| *keep && cursor.is_over(e.bounds()));
            if over && !on_submenu {
                self.close(shell);
            }
        }
        if over {
            shell.capture_event();
        }
    }

    fn mouse_interaction(&self, layout: Layout<'_>, cursor: mouse::Cursor, renderer: &Renderer) -> mouse::Interaction {
        let mut children = layout.children();
        let main = children.next().map(|m| {
            self.main.as_widget().mouse_interaction(self.main_tree, m, cursor, &m.bounds(), renderer)
        });
        let fly = match (self.flyout.as_ref(), children.next()) {
            (Some(((_, f), tree)), Some(l)) => Some(f.as_widget().mouse_interaction(tree, l, cursor, &l.bounds(), renderer)),
            _ => None,
        };
        fly.filter(|i| *i != mouse::Interaction::default()).or(main).unwrap_or_default()
    }
}
