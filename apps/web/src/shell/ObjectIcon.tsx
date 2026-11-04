import React from 'react';
import {Briefcase, Buildings, CalendarBlank, ChartBar, CheckSquare, Cube, Pulse, SquaresFour, User, UsersThree} from '@phosphor-icons/react';
import type {NavigationContribution} from './types';

type IconComponent = React.ComponentType<{weight?: 'regular' | 'fill'; 'aria-hidden'?: boolean}>;

/**
 * The validated icon identifiers a feature may name in its navigation
 * contribution, each mapped to a maintained glyph and an object colour token.
 * Unknown identifiers fall back to the generic feature tile; plugin-provided
 * markup is never executed.
 */
const objectIcons: Record<string, {icon: IconComponent; tone: string}> = {
  people: {icon: User, tone: 'people'},
  users: {icon: UsersThree, tone: 'people'},
  contacts: {icon: UsersThree, tone: 'people'},
  organisations: {icon: Buildings, tone: 'organisations'},
  activity: {icon: Pulse, tone: 'activity'},
  tasks: {icon: CheckSquare, tone: 'tasks'},
  cases: {icon: Briefcase, tone: 'cases'},
  calendar: {icon: CalendarBlank, tone: 'calendar'},
  // Workspace objects from the kernel object engine (ADR 0015).
  cube: {icon: Cube, tone: 'app'},
  // Saved reports (ADR 0027).
  reports: {icon: ChartBar, tone: 'app'},
};

export function objectTone(icon?: string): string {
  return objectIcons[icon ?? '']?.tone ?? 'app';
}

export function ObjectGlyph({icon}: {icon?: string}) {
  const Icon = objectIcons[icon ?? '']?.icon ?? SquaresFour;
  return <Icon aria-hidden={true} weight="regular" />;
}

/** A coloured object tile: the same mark for a feature in the rail, launcher, headers and records. */
export function ObjectTile({item, size = 'medium'}: {item: Pick<NavigationContribution, 'icon'>; size?: 'small' | 'medium' | 'large'}) {
  return <span className={`object-tile object-tile--${size} object-tile--${objectTone(item.icon)}`} aria-hidden="true"><ObjectGlyph icon={item.icon} /></span>;
}
