import type {NavigationContribution} from './types';

/** The contribution whose route is the longest prefix of the current route, so nested app routes resolve to the right app. */
export function activeContributionFor(navigation: readonly NavigationContribution[], route: string): NavigationContribution | undefined {
  return navigation
    .filter(item => route === item.route || route.startsWith(`${item.route}/`))
    .sort((left, right) => right.route.length - left.route.length)[0];
}
