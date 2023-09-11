import { Link } from "react-router-dom";
import { Account } from "../../../bindings/Account";

interface Props {
	accounts: Account[],
}
export default function AccountsList({ accounts }: Props) {
	return <ul className="flex flex-col space-y-2">
		{accounts.map(account => (
			<li key={ account.id }>
				<Link className="flex space-x-3 p-2 bg-white/50 border-b border-b-slate-50 font-medium text-xs text-slate-600 items-center hover:text-purple-900" to={`/accounts/${account.id}`}>
					{ account.icon &&
						<img src={account.icon} className="w-6" />
					}
					<span className="flex flex-col">
						<span>{account.name || '(untitled)'}</span>
						<span className="text-gray-400">{ [ account.iban || account.bban || account.bic, account.owner_name, account.product, account.currency, account.details ].join( ' ' )}</span>
					</span>
					<span className="flex-1"></span>
					{ !! account.balance &&
						<span className="flex flex-col w-24">
							<span className="text-xs text-gray-400">Balance</span>
							<span className="">{ account.balance } { account.currency }</span>
						</span>
					}
				</Link></li>
		))}
	</ul>
}
