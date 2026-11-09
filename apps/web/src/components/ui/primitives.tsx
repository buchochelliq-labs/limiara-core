import React, {useEffect, useId, useRef} from 'react';
import {
  CheckCircle,
  Info,
  SpinnerGap,
  Warning,
  WarningCircle,
  UsersThree,
  X,
} from '@phosphor-icons/react';
import './primitives.css';

const cx = (...values: Array<string | false | null | undefined>) => values.filter(Boolean).join(' ');

export type ButtonProps = React.ButtonHTMLAttributes<HTMLButtonElement> & {
  variant?: 'primary' | 'secondary' | 'quiet' | 'danger';
  size?: 'small' | 'medium';
  loading?: boolean;
};

export const Button = React.forwardRef<HTMLButtonElement, ButtonProps>(function Button({variant = 'primary', size = 'medium', loading = false, className, children, disabled, ...props}, ref) {
  return <button ref={ref} className={cx('ui-button', `ui-button--${variant}`, `ui-button--${size}`, className)} disabled={disabled || loading} aria-busy={loading || undefined} {...props}>
    {loading && <SpinnerGap aria-hidden="true" className="ui-spin" weight="bold" />}
    <span>{children}</span>
  </button>;
});

export type IconButtonProps = Omit<React.ButtonHTMLAttributes<HTMLButtonElement>, 'aria-label'> & {
  label: string;
  icon: React.ReactNode;
  variant?: 'secondary' | 'quiet' | 'danger';
};

export function IconButton({label, icon, variant = 'secondary', className, ...props}: IconButtonProps) {
  return <button className={cx('ui-icon-button', `ui-icon-button--${variant}`, className)} aria-label={label} title={label} {...props}>{icon}</button>;
}

type FieldShared = {
  label: string;
  description?: string;
  error?: string;
  hideLabel?: boolean;
};

export type TextFieldProps = React.InputHTMLAttributes<HTMLInputElement> & FieldShared;

export function TextField({label, description, error, hideLabel = false, id: providedId, className, ...props}: TextFieldProps) {
  const generatedId = useId();
  const id = providedId ?? generatedId;
  const descriptionId = description ? `${id}-description` : undefined;
  const errorId = error ? `${id}-error` : undefined;
  return <div className="ui-field">
    <label className={cx('ui-field__label', hideLabel && 'sr-only')} htmlFor={id}>{label}</label>
    {description && <span className="ui-field__description" id={descriptionId}>{description}</span>}
    <input className={cx('ui-field__control', error && 'ui-field__control--error', className)} id={id} aria-invalid={error ? true : undefined} aria-describedby={cx(descriptionId, errorId) || undefined} {...props} />
    {error && <span className="ui-field__error" id={errorId}><WarningCircle aria-hidden="true" weight="fill" />{error}</span>}
  </div>;
}

export type SelectProps = React.SelectHTMLAttributes<HTMLSelectElement> & FieldShared;

export function Select({label, description, error, hideLabel = false, id: providedId, className, children, ...props}: SelectProps) {
  const generatedId = useId();
  const id = providedId ?? generatedId;
  const descriptionId = description ? `${id}-description` : undefined;
  const errorId = error ? `${id}-error` : undefined;
  return <div className="ui-field">
    <label className={cx('ui-field__label', hideLabel && 'sr-only')} htmlFor={id}>{label}</label>
    {description && <span className="ui-field__description" id={descriptionId}>{description}</span>}
    <select className={cx('ui-field__control', 'ui-field__select', error && 'ui-field__control--error', className)} id={id} aria-invalid={error ? true : undefined} aria-describedby={cx(descriptionId, errorId) || undefined} {...props}>{children}</select>
    {error && <span className="ui-field__error" id={errorId}><WarningCircle aria-hidden="true" weight="fill" />{error}</span>}
  </div>;
}

export type BadgeProps = React.HTMLAttributes<HTMLSpanElement> & {
  variant?: 'neutral' | 'selected' | 'success' | 'warning' | 'danger';
};

export function Badge({variant = 'neutral', className, children, ...props}: BadgeProps) {
  return <span className={cx('ui-badge', `ui-badge--${variant}`, className)} {...props}>{children}</span>;
}

export type AvatarProps = {
  name: string;
  src?: string;
  size?: 'small' | 'medium' | 'large';
};

export function Avatar({name, src, size = 'medium'}: AvatarProps) {
  const initials = name.trim().split(/\s+/).slice(0, 2).map(part => part[0]).join('').toUpperCase();
  const palette = (Array.from(name).reduce((sum, character) => sum + character.charCodeAt(0), 0) + 1) % 4;
  return <span className={cx('ui-avatar', `ui-avatar--${size}`, `ui-avatar--palette-${palette}`)}>
    {src ? <img alt="" src={src} /> : <span aria-hidden="true">{initials}</span>}
    <span className="sr-only">{name}</span>
  </span>;
}

export type AlertProps = React.HTMLAttributes<HTMLDivElement> & {
  variant?: 'info' | 'success' | 'warning' | 'danger';
  title: string;
};

const alertIcons = {info: Info, success: CheckCircle, warning: Warning, danger: WarningCircle};

export function Alert({variant = 'info', title, children, className, ...props}: AlertProps) {
  const AlertIcon = alertIcons[variant];
  const role = variant === 'danger' ? 'alert' : 'status';
  return <div className={cx('ui-alert', `ui-alert--${variant}`, className)} role={role} {...props}>
    <AlertIcon className="ui-alert__icon" aria-hidden="true" weight="fill" />
    <div><strong>{title}</strong>{children && <div className="ui-alert__body">{children}</div>}</div>
  </div>;
}

export type EmptyStateProps = {
  title: string;
  description: string;
  action?: React.ReactNode;
  icon?: React.ReactNode;
};

export function EmptyState({title, description, action, icon = <UsersThree aria-hidden="true" />}: EmptyStateProps) {
  return <div className="ui-empty-state">
    <div className="ui-empty-state__icon">{icon}</div>
    <h3>{title}</h3>
    <p>{description}</p>
    {action && <div className="ui-empty-state__action">{action}</div>}
  </div>;
}

export type SkeletonProps = {label?: string; lines?: number};

export function Skeleton({label = 'Loading content', lines = 3}: SkeletonProps) {
  return <div className="ui-skeleton" role="status" aria-label={label}>
    <span className="ui-skeleton__avatar" aria-hidden="true" />
    <span className="ui-skeleton__content" aria-hidden="true">
      {Array.from({length: lines}, (_, index) => <span className="ui-skeleton__line" key={index} />)}
    </span>
  </div>;
}

export type ModalProps = {
  title: string;
  /** Called on Escape, the close button or a click on the backdrop. */
  onClose: () => void;
  /** Ignores closing while work is in flight, such as a save. */
  busy?: boolean;
  children: React.ReactNode;
};

const focusable = 'a[href], button:not([disabled]), input:not([disabled]):not([type="hidden"]), select:not([disabled]), textarea:not([disabled]), [tabindex]:not([tabindex="-1"])';

/**
 * A modal dialog: focus moves to its first control, Tab stays inside it,
 * Escape closes it, and focus returns to whatever opened it.
 */
export function Modal({title, onClose, busy = false, children}: ModalProps) {
  const titleId = useId();
  const panelRef = useRef<HTMLDivElement>(null);
  const close = useRef(onClose);
  close.current = onClose;
  const closable = useRef(!busy);
  closable.current = !busy;

  useEffect(() => {
    const opener = document.activeElement instanceof HTMLElement ? document.activeElement : null;
    const panel = panelRef.current;
    const first = panel?.querySelector('.ui-modal__body')?.querySelector<HTMLElement>(focusable) ?? panel;
    first?.focus();
    const previousOverflow = document.body.style.overflow;
    document.body.style.overflow = 'hidden';
    const onKey = (event: KeyboardEvent) => {
      if (event.key === 'Escape') {
        event.preventDefault();
        if (closable.current) close.current();
        return;
      }
      if (event.key !== 'Tab' || !panel) return;
      const items = Array.from(panel.querySelectorAll<HTMLElement>(focusable));
      if (!items.length) { event.preventDefault(); return; }
      const head = items[0];
      const tail = items[items.length - 1];
      if (event.shiftKey && (document.activeElement === head || document.activeElement === panel)) { event.preventDefault(); tail?.focus(); }
      else if (!event.shiftKey && document.activeElement === tail) { event.preventDefault(); head?.focus(); }
    };
    document.addEventListener('keydown', onKey);
    return () => {
      document.removeEventListener('keydown', onKey);
      document.body.style.overflow = previousOverflow;
      if (opener?.isConnected) opener.focus();
    };
  }, []);

  return <div className="ui-modal" onMouseDown={event => { if (event.target === event.currentTarget && closable.current) close.current(); }}>
    <div ref={panelRef} className="ui-modal__panel" role="dialog" aria-modal="true" aria-labelledby={titleId} tabIndex={-1}>
      <header className="ui-modal__header">
        <h2 id={titleId}>{title}</h2>
        <button type="button" className="ui-modal__close" aria-label="Close" title="Close" onClick={() => { if (closable.current) close.current(); }} disabled={busy}><X aria-hidden="true" /></button>
      </header>
      <div className="ui-modal__body">{children}</div>
    </div>
  </div>;
}
